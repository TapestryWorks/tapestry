use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{debug, warn};

use tapestry_protocol::{
    Command, ErrorEvent, EventMsg, SessionConfiguredEvent, Submission, SubmissionId, ThreadId,
    TurnAbortReason, TurnAbortedEvent, TurnId,
};

use crate::agent::AgentLoop;
use crate::clock::{Clock, SystemClock};
use crate::config::ThreadConfig;
use crate::error::CoreError;
use crate::formatter::formatter_trait::ConversationProjection;
use crate::model::ModelClient;
use crate::recorder::Recorder;
use crate::store::{EventStore, EventSubscription, InMemoryEventStore};
use crate::tool::ToolRegistry;

const DEFAULT_CHANNEL_CAPACITY: usize = 64;

/// Per-thread session state owned by the worker task. History is not kept
/// here: it is the projection of the event log, owned by the [`Recorder`].
#[derive(Default)]
struct Session {
    config: ThreadConfig,
    configured: bool,
    active_turn: Option<TurnId>,
}

fn validate_user_name(name: &str) -> Result<(), CoreError> {
    if name.trim().is_empty() {
        return Err(CoreError::InvalidInput("name must not be empty".into()));
    }
    if name.contains(['\n', '\r']) {
        return Err(CoreError::InvalidInput(
            "name must not contain line breaks".into(),
        ));
    }
    Ok(())
}

/// Handle for submitting commands to a thread and subscribing to its events.
#[derive(Clone)]
pub struct ThreadHandle {
    thread_id: ThreadId,
    submission_tx: mpsc::Sender<Submission>,
    store: Arc<dyn EventStore>,
}

impl ThreadHandle {
    pub fn thread_id(&self) -> ThreadId {
        self.thread_id
    }

    pub async fn submit(&self, command: Command) -> Result<SubmissionId, CoreError> {
        let submission = Submission::new(command);
        let id = submission.id;
        self.submission_tx
            .send(submission)
            .await
            .map_err(|_| CoreError::ChannelClosed)?;
        Ok(id)
    }

    /// Follow the thread's event log from just after `after_log_position` (0 = from the
    /// start). Every subscription has its own cursor and sees every event.
    pub async fn subscribe(&self, after_log_position: u64) -> Result<EventSubscription, CoreError> {
        self.store
            .subscribe(self.thread_id, after_log_position)
            .await
    }
}

/// Configures and starts a [`TapestryThread`].
pub struct ThreadBuilder {
    model: Arc<dyn ModelClient>,
    tools: Arc<ToolRegistry>,
    clock: Arc<dyn Clock>,
    store: Arc<dyn EventStore>,
    agent_loop: Option<AgentLoop>,
}

impl ThreadBuilder {
    pub fn clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Where the thread's events are stored; defaults to [`InMemoryEventStore`].
    pub fn event_store(mut self, store: Arc<dyn EventStore>) -> Self {
        self.store = store;
        self
    }

    pub fn agent_loop(mut self, agent_loop: AgentLoop) -> Self {
        self.agent_loop = Some(agent_loop);
        self
    }

    /// Start a new thread. Must be called within a Tokio runtime.
    pub fn spawn(self) -> (TapestryThread, ThreadHandle) {
        self.start(ThreadId::new(), ConversationProjection::new())
    }

    /// Continue an existing thread from its event log: history is folded from
    /// the log and new events continue their log positions. Settings are not
    /// stored in the log, so the resumed thread is unconfigured until
    /// `UpdateSettings`.
    pub async fn resume(
        self,
        thread_id: ThreadId,
    ) -> Result<(TapestryThread, ThreadHandle), CoreError> {
        let events = self.store.read_after(thread_id, 0).await?;
        let projection = ConversationProjection::fold(&events);
        Ok(self.start(thread_id, projection))
    }

    fn start(
        self,
        thread_id: ThreadId,
        projection: ConversationProjection,
    ) -> (TapestryThread, ThreadHandle) {
        let (submission_tx, submission_rx) = mpsc::channel(DEFAULT_CHANNEL_CAPACITY);
        let handle = ThreadHandle {
            thread_id,
            submission_tx,
            store: self.store.clone(),
        };
        let recorder =
            Recorder::with_projection(self.store, self.clock.clone(), thread_id, projection);
        let agent_loop = match self.agent_loop {
            Some(agent_loop) => Arc::new(agent_loop),
            None => Arc::new(AgentLoop::with_clock(self.model, self.tools, self.clock)),
        };
        tokio::spawn(run_thread_loop(submission_rx, recorder, agent_loop));

        let thread = TapestryThread {
            thread_id,
            handle: handle.clone(),
        };
        (thread, handle)
    }
}

/// A long-lived agent thread: a submission queue in, an event log out.
pub struct TapestryThread {
    thread_id: ThreadId,
    handle: ThreadHandle,
}

impl TapestryThread {
    pub fn builder(model: Arc<dyn ModelClient>, tools: Arc<ToolRegistry>) -> ThreadBuilder {
        ThreadBuilder {
            model,
            tools,
            clock: Arc::new(SystemClock),
            store: Arc::new(InMemoryEventStore::new()),
            agent_loop: None,
        }
    }

    pub fn spawn(model: Arc<dyn ModelClient>, tools: Arc<ToolRegistry>) -> (Self, ThreadHandle) {
        Self::builder(model, tools).spawn()
    }

    pub fn spawn_with_clock(
        model: Arc<dyn ModelClient>,
        tools: Arc<ToolRegistry>,
        clock: Arc<dyn Clock>,
    ) -> (Self, ThreadHandle) {
        Self::builder(model, tools).clock(clock).spawn()
    }

    pub fn thread_id(&self) -> ThreadId {
        self.thread_id
    }

    pub fn handle(&self) -> ThreadHandle {
        self.handle.clone()
    }
}

async fn run_thread_loop(
    mut submission_rx: mpsc::Receiver<Submission>,
    mut recorder: Recorder,
    agent_loop: Arc<AgentLoop>,
) {
    let mut session = Session::default();

    while let Some(submission) = submission_rx.recv().await {
        debug!(submission_id = %submission.id, "processing submission");
        recorder.begin_submission(submission.id);

        let result = process_submission(
            agent_loop.as_ref(),
            &mut session,
            &mut recorder,
            submission.command,
        )
        .await;

        match result {
            Ok(()) => {}
            // Record an error with the turn id of the failed submission when
            // the store still accepts writes; otherwise keep the append error.
            Err(err) => {
                let message = err.to_string();
                if let Err(err) = recorder
                    .record(EventMsg::Error(ErrorEvent { message }))
                    .await
                {
                    warn!(?err, submission_id = %submission.id, "failed to record error");
                }
            }
        }
    }
}

async fn process_submission(
    agent_loop: &AgentLoop,
    session: &mut Session,
    recorder: &mut Recorder,
    command: Command,
) -> Result<(), CoreError> {
    match command {
        Command::UpdateSettings { settings } => {
            session.config.apply_settings(&settings);
            if !session.configured {
                session.configured = true;
                let thread_id = recorder.thread_id();
                recorder
                    .record(EventMsg::SessionConfigured(SessionConfiguredEvent {
                        thread_id,
                        model: session.config.model.clone(),
                    }))
                    .await?;
            }
        }
        Command::Interrupt => {
            recorder.set_turn(session.active_turn.take());
            recorder
                .record(EventMsg::TurnAborted(TurnAbortedEvent {
                    reason: TurnAbortReason::Interrupted,
                    message: Some("interrupted by client".into()),
                }))
                .await?;
        }
        Command::StartTurn {
            content,
            name,
            turn_id,
        } => {
            if !session.configured {
                return Err(CoreError::NotConfigured);
            }
            if session.active_turn.is_some() {
                return Err(CoreError::TurnInProgress);
            }
            if let Some(name) = &name {
                validate_user_name(name)?;
            }

            let turn_id = turn_id.unwrap_or_else(TurnId::new);
            session.active_turn = Some(turn_id);
            let run_result = agent_loop
                .forward(recorder, turn_id, content, name, &session.config)
                .await;
            session.active_turn.take();
            run_result?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use tapestry_protocol::{
        ConversationItem, Event, EventMsg, FinishReason, ModelRequest, ModelResponse,
        ThreadSettings,
    };

    use super::*;
    use crate::agent::{Agent, AgentBindings, StepOutput, TurnOutcome, TurnStep};
    use crate::clock::FixedClock;
    use crate::formatter::prompt::render_system_reminder;
    use crate::formatter::{Formatter, OpenAiFormatter};
    use crate::middleware::{HookCtx, Middleware, ToolDecision};
    use crate::model::MockModelClient;
    use crate::store::testing::FlakyStore;
    use crate::store::NewEvent;
    use crate::tool::{EchoTool, ToolRegistry};

    fn request_history(request: &ModelRequest) -> Vec<ConversationItem> {
        OpenAiFormatter.events_to_items(&request.events)
    }

    fn test_tools() -> Arc<ToolRegistry> {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        Arc::new(registry)
    }

    /// A thread handle plus one subscription that reads from the start.
    struct Client {
        handle: ThreadHandle,
        events: EventSubscription,
    }

    impl Client {
        async fn new(handle: ThreadHandle) -> Self {
            Self::after(handle, 0).await
        }

        async fn after(handle: ThreadHandle, log_position: u64) -> Self {
            let events = handle.subscribe(log_position).await.unwrap();
            Self { handle, events }
        }

        async fn spawn(model: Arc<dyn ModelClient>) -> Self {
            Self::new(TapestryThread::spawn(model, test_tools()).1).await
        }

        async fn send(&mut self, command: Command) -> Vec<Event> {
            let id = self.handle.submit(command).await.unwrap();
            tokio::time::timeout(Duration::from_secs(5), self.events.collect_events_for(id))
                .await
                .expect("no terminal event")
                .unwrap()
        }

        async fn settings(&self, settings: ThreadSettings) {
            self.handle
                .submit(Command::UpdateSettings { settings })
                .await
                .unwrap();
        }

        async fn configure(&mut self) -> Vec<Event> {
            self.send(Command::UpdateSettings {
                settings: ThreadSettings {
                    model: Some("mock-model".into()),
                    ..Default::default()
                },
            })
            .await
        }

        async fn configure_without_time_context(&mut self) {
            self.configure().await;
            self.settings(ThreadSettings {
                include_time_context: Some(false),
                ..Default::default()
            })
            .await;
        }

        async fn turn(&mut self, msg: &str, user_name: Option<&str>) -> Vec<Event> {
            self.send(Command::StartTurn {
                content: msg.into(),
                name: user_name.map(str::to_string),
                turn_id: None,
            })
            .await
        }
    }

    fn agent_text(events: &[Event]) -> String {
        events
            .iter()
            .filter_map(|e| match &e.msg {
                EventMsg::AssistantMessage(m) => m.content.clone(),
                _ => None,
            })
            .collect()
    }

    /// Echoes the number of history items it sees.
    struct HistoryCountModel;

    #[async_trait::async_trait]
    impl ModelClient for HistoryCountModel {
        async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, CoreError> {
            Ok(ModelResponse {
                content: Some(format!("items={}", request_history(&request).len())),
                tool_calls: vec![],
                usage: Default::default(),
            })
        }
    }

    /// Echoes the system prompt it receives.
    struct PromptEcho;

    #[async_trait::async_trait]
    impl ModelClient for PromptEcho {
        async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, CoreError> {
            Ok(ModelResponse {
                content: request.system_prompt,
                tool_calls: vec![],
                usage: Default::default(),
            })
        }
    }

    /// Behaves like [`MockModelClient`] and records every request.
    #[derive(Default)]
    struct RecordingModel {
        requests: Mutex<Vec<ModelRequest>>,
    }

    #[async_trait::async_trait]
    impl ModelClient for RecordingModel {
        async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, CoreError> {
            self.requests.lock().unwrap().push(request.clone());
            MockModelClient.complete(request).await
        }
    }

    struct FailingModel;

    #[async_trait::async_trait]
    impl ModelClient for FailingModel {
        async fn complete(&self, _: ModelRequest) -> Result<ModelResponse, CoreError> {
            Err(CoreError::Model("down".into()))
        }
    }

    struct ThreadMiddleware {
        tool_calls: Arc<AtomicUsize>,
        fail_before_model: bool,
        fail_after_step: bool,
    }

    #[async_trait::async_trait]
    impl Middleware for ThreadMiddleware {
        async fn before_model(
            &self,
            _ctx: &HookCtx<'_>,
            _events: &[tapestry_protocol::Event],
        ) -> Result<(), CoreError> {
            if self.fail_before_model {
                return Err(CoreError::Tool("middleware failure".into()));
            }
            Ok(())
        }

        async fn before_tool(
            &self,
            _ctx: &HookCtx<'_>,
            _call: &mut tapestry_protocol::ToolCall,
        ) -> Result<ToolDecision, CoreError> {
            self.tool_calls.fetch_add(1, Ordering::SeqCst);
            Ok(ToolDecision::Continue)
        }

        async fn after_step(
            &self,
            _ctx: &HookCtx<'_>,
            _output: &StepOutput<'_>,
            _next_step: &mut TurnStep,
        ) -> Result<(), CoreError> {
            if self.fail_after_step {
                return Err(CoreError::Tool("middleware after_step failure".into()));
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn thread_configure_and_turn() {
        let mut c = Client::spawn(Arc::new(MockModelClient)).await;
        let config_events = c
            .send(Command::UpdateSettings {
                settings: ThreadSettings {
                    model: Some("mock-model".into()),
                    system_prompt: Some("test".into()),
                    ..Default::default()
                },
            })
            .await;
        match &config_events.last().unwrap().msg {
            EventMsg::SessionConfigured(e) => assert_eq!(e.thread_id, c.handle.thread_id()),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(config_events.last().unwrap().turn_id, None);

        let turn_events = c.turn("hello", None).await;
        assert!(turn_events
            .iter()
            .any(|e| matches!(e.msg, EventMsg::AssistantMessage(_))));
        assert!(matches!(
            turn_events.last().unwrap().msg,
            EventMsg::TurnComplete(_)
        ));
    }

    #[tokio::test]
    async fn thread_rejects_turn_before_configure() {
        let mut c = Client::spawn(Arc::new(MockModelClient)).await;
        let events = c.turn("hello", None).await;
        let last = events.last().unwrap();
        assert!(matches!(last.msg, EventMsg::Error(_)));
        assert_eq!(last.turn_id, None);
    }

    #[tokio::test]
    async fn first_settings_emit_session_configured_and_later_ones_stay_silent() {
        let mut c = Client::spawn(Arc::new(PromptEcho)).await;
        let events = c
            .send(Command::UpdateSettings {
                settings: ThreadSettings {
                    model: Some("m1".into()),
                    system_prompt: Some("sys".into()),
                    ..Default::default()
                },
            })
            .await;
        match events.last().map(|e| &e.msg) {
            Some(EventMsg::SessionConfigured(e)) => assert_eq!(e.model, "m1"),
            other => panic!("unexpected {other:?}"),
        }

        c.settings(ThreadSettings {
            model: Some("m2".into()),
            ..Default::default()
        })
        .await;
        let events = c.turn("x", None).await;
        assert!(!events
            .iter()
            .any(|e| matches!(e.msg, EventMsg::SessionConfigured(_))));
        assert_eq!(agent_text(&events), "sys");
    }

    #[tokio::test]
    async fn history_persists_across_turns() {
        let mut c = Client::spawn(Arc::new(HistoryCountModel)).await;
        c.configure_without_time_context().await;
        assert_eq!(agent_text(&c.turn("one", None).await), "items=1");
        // user, assistant, user
        assert_eq!(agent_text(&c.turn("two", None).await), "items=3");
    }

    #[tokio::test]
    async fn named_user_is_rendered_for_model() {
        let mut c = Client::spawn(Arc::new(MockModelClient)).await;
        c.configure().await;
        let events = c.turn("hi", Some("alice")).await;
        assert_eq!(agent_text(&events), "Echo: alice: hi");
    }

    #[tokio::test]
    async fn invalid_user_name_errors_and_leaves_history() {
        let mut c = Client::spawn(Arc::new(HistoryCountModel)).await;
        c.configure_without_time_context().await;
        for bad in ["", "   ", "a\nb"] {
            let events = c.turn("hi", Some(bad)).await;
            assert!(
                matches!(events.last().unwrap().msg, EventMsg::Error(_)),
                "{bad:?}"
            );
            assert!(!events
                .iter()
                .any(|e| matches!(e.msg, EventMsg::TurnStarted(_))));
        }
        assert_eq!(agent_text(&c.turn("ok", None).await), "items=1");
    }

    #[tokio::test]
    async fn interrupt_reports_enum_reason() {
        let mut c = Client::spawn(Arc::new(MockModelClient)).await;
        let events = c.send(Command::Interrupt).await;
        match &events.last().unwrap().msg {
            EventMsg::TurnAborted(a) => assert_eq!(a.reason, TurnAbortReason::Interrupted),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn agent_name_setting_updates_system_prompt() {
        let mut c = Client::spawn(Arc::new(PromptEcho)).await;
        c.configure().await;
        assert_eq!(agent_text(&c.turn("x", None).await), "");

        c.settings(ThreadSettings {
            agent_name: Some("planner".into()),
            ..Default::default()
        })
        .await;
        let text = agent_text(&c.turn("x", None).await);
        assert!(text.contains("planner"));
    }

    #[tokio::test]
    async fn thread_injects_time_context_once_per_day_and_honors_setting() {
        /// Echoes how many time-context entries the request history holds.
        struct ContextCountModel;
        #[async_trait::async_trait]
        impl ModelClient for ContextCountModel {
            async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, CoreError> {
                let n = request_history(&request)
                    .iter()
                    .filter(|i| matches!(i, ConversationItem::EnvironmentContext(_)))
                    .count();
                Ok(ModelResponse {
                    content: Some(n.to_string()),
                    tool_calls: vec![],
                    usage: Default::default(),
                })
            }
        }

        let clock = Arc::new(FixedClock::new("2026-11-04", "UTC"));
        let (_t, handle) = TapestryThread::spawn_with_clock(
            Arc::new(ContextCountModel),
            test_tools(),
            clock.clone(),
        );
        let mut c = Client::new(handle).await;
        c.configure().await;
        assert_eq!(agent_text(&c.turn("a", None).await), "1");
        assert_eq!(agent_text(&c.turn("b", None).await), "1");
        clock.set("2026-11-05", "UTC");
        assert_eq!(agent_text(&c.turn("c", None).await), "2");

        c.settings(ThreadSettings {
            include_time_context: Some(false),
            ..Default::default()
        })
        .await;
        // Disabling after a send appends one "unavailable" entry.
        assert_eq!(agent_text(&c.turn("d", None).await), "3");
        assert_eq!(agent_text(&c.turn("e", None).await), "3");
    }

    #[tokio::test]
    async fn turn_error_carries_turn_id_and_repairs_history() {
        let mut c = Client::spawn(Arc::new(FailingModel)).await;
        c.configure().await;
        let events = c.turn("hi", None).await;
        let started = events
            .iter()
            .find(|e| matches!(e.msg, EventMsg::TurnStarted(_)))
            .unwrap();
        let error = events.last().unwrap();
        assert!(matches!(error.msg, EventMsg::Error(_)));
        assert!(error.turn_id.is_some());
        assert_eq!(error.turn_id, started.turn_id);
    }

    #[tokio::test]
    async fn after_step_errors_are_recorded_for_the_submission() {
        let model: Arc<dyn ModelClient> = Arc::new(MockModelClient);
        let agent = AgentLoop::new(model.clone(), test_tools()).with_middleware(Arc::new(
            ThreadMiddleware {
                tool_calls: Arc::new(AtomicUsize::new(0)),
                fail_before_model: false,
                fail_after_step: true,
            },
        ));
        let (_thread, handle) = TapestryThread::builder(model, test_tools())
            .agent_loop(agent)
            .spawn();
        let mut client = Client::new(handle.clone()).await;
        client.configure_without_time_context().await;
        let submission = handle
            .submit(Command::StartTurn {
                content: "hello".into(),
                name: None,
                turn_id: None,
            })
            .await
            .unwrap();
        let events = tokio::time::timeout(
            Duration::from_secs(5),
            client.events.collect_events_for(submission),
        )
        .await
        .expect("no terminal event")
        .unwrap();

        let error = events
            .iter()
            .find(|event| matches!(event.msg, EventMsg::Error(_)))
            .unwrap();
        assert_eq!(error.id, submission);
        assert!(error.turn_id.is_some());
        assert!(matches!(
            &error.msg,
            EventMsg::Error(error) if error.message.contains("after_step")
        ));
        assert!(!events
            .iter()
            .any(|event| matches!(event.msg, EventMsg::TurnComplete(_))));
    }

    #[tokio::test]
    async fn middleware_state_persists_across_turns_without_extra_events() {
        let model: Arc<dyn ModelClient> = Arc::new(RecordingModel::default());
        let tool_calls = Arc::new(AtomicUsize::new(0));
        let agent = AgentLoop::new(model.clone(), test_tools()).with_middleware(Arc::new(
            ThreadMiddleware {
                tool_calls: tool_calls.clone(),
                fail_before_model: false,
                fail_after_step: false,
            },
        ));
        let (_thread, handle) = TapestryThread::builder(model, test_tools())
            .agent_loop(agent)
            .spawn();
        let mut client = Client::new(handle).await;
        client.configure_without_time_context().await;

        let first = client.turn("tool:echo", None).await;
        let second = client.turn("tool:echo", None).await;

        assert_eq!(tool_calls.load(Ordering::SeqCst), 2);
        for events in [first, second] {
            assert_eq!(events.len(), 14);
            assert!(!events
                .iter()
                .any(|event| matches!(event.msg, EventMsg::Error(_))));
            assert!(matches!(
                events.last().unwrap().msg,
                EventMsg::TurnComplete(_)
            ));
        }
    }

    struct ThreadPolicy {
        bindings: AgentBindings,
    }

    #[async_trait::async_trait]
    impl Agent for ThreadPolicy {
        fn bindings(&self) -> &AgentBindings {
            &self.bindings
        }

        fn bindings_mut(&mut self) -> &mut AgentBindings {
            &mut self.bindings
        }

        async fn forward(
            &mut self,
            _ctx: &mut crate::agent::TurnContext<'_>,
        ) -> Result<TurnOutcome, CoreError> {
            Ok(TurnOutcome {
                finish: FinishReason::ExceedMaxIters,
                last_agent_message: Some("selected by policy".into()),
            })
        }
    }

    #[tokio::test]
    async fn thread_uses_custom_forward_from_agent_loop_factory() {
        let model: Arc<dyn ModelClient> = Arc::new(MockModelClient);
        let loop_with_policy = AgentLoop::new(model.clone(), test_tools())
            .with_agent_factory(|bindings, _| Box::new(ThreadPolicy { bindings }));
        let (_thread, handle) = TapestryThread::builder(model, test_tools())
            .agent_loop(loop_with_policy)
            .spawn();
        let mut client = Client::new(handle.clone()).await;
        client.configure_without_time_context().await;
        let submission = handle
            .submit(Command::StartTurn {
                content: "hello".into(),
                name: None,
                turn_id: None,
            })
            .await
            .unwrap();
        let events = tokio::time::timeout(
            Duration::from_secs(5),
            client.events.collect_events_for(submission),
        )
        .await
        .expect("no terminal event")
        .unwrap();

        assert!(events.iter().any(|event| {
            event.id == submission
                && matches!(
                    &event.msg,
                    EventMsg::TurnComplete(complete)
                        if complete.finish_reason == FinishReason::ExceedMaxIters
                            && complete.last_agent_message.as_deref() == Some("selected by policy")
                )
        }));
        assert!(!events
            .iter()
            .any(|event| matches!(event.msg, EventMsg::Error(_))));
    }

    #[tokio::test]
    async fn two_subscribers_each_receive_the_whole_turn() {
        let (_t, handle) = TapestryThread::spawn(Arc::new(MockModelClient), test_tools());
        let mut first = Client::new(handle.clone()).await;
        let mut second = handle.subscribe(0).await.unwrap();
        first.configure().await;
        let id = handle
            .submit(Command::StartTurn {
                content: "tool:echo".into(),
                name: None,
                turn_id: None,
            })
            .await
            .unwrap();
        let (a, b) = tokio::join!(
            first.events.collect_events_for(id),
            second.collect_events_for(id)
        );
        let (a, b) = (a.unwrap(), b.unwrap());
        // `first` already consumed the configuration events.
        assert_eq!(&b[b.len() - a.len()..], a.as_slice());
        assert!(a.iter().any(|e| matches!(e.msg, EventMsg::ToolCallEnd(_))));
        assert!(matches!(a.last().unwrap().msg, EventMsg::TurnComplete(_)));
        let positions: Vec<_> = b.iter().map(|e| e.log_position).collect();
        assert_eq!(positions, (1..=b.len() as u64).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn store_failure_ends_submission_and_thread_keeps_running() {
        let store = Arc::new(FlakyStore::default());
        let (_t, handle) = TapestryThread::builder(Arc::new(MockModelClient), test_tools())
            .event_store(store.clone())
            .spawn();
        let mut c = Client::new(handle.clone()).await;
        c.configure_without_time_context().await;

        // turn_started, user, req_start, assistant(tool call), then fail.
        store.fail_after(4);
        let failed = handle
            .submit(Command::StartTurn {
                content: "tool:echo".into(),
                name: None,
                turn_id: None,
            })
            .await
            .unwrap();
        // Submissions are serial, so once the next one completes the failed
        // one has finished.
        store.heal();
        let events = c.turn("again", None).await;
        assert!(matches!(
            events.last().unwrap().msg,
            EventMsg::TurnComplete(_)
        ));
        assert!(!events
            .iter()
            .any(|e| e.id == failed && matches!(e.msg, EventMsg::Error(_))));

        let log = store.read_after(handle.thread_id(), 0).await.unwrap();
        let history = ConversationProjection::fold(&log).messages();
        let calls: Vec<_> = history
            .iter()
            .flat_map(|i| match i {
                ConversationItem::Assistant { tool_calls, .. } => tool_calls.clone(),
                _ => vec![],
            })
            .collect();
        assert_eq!(calls.len(), 1);
        assert!(history
            .iter()
            .any(|i| matches!(i, ConversationItem::ToolResult(r) if r.call_id == calls[0].id)));
    }

    #[tokio::test]
    async fn event_snapshot_read_failure_is_reported_without_calling_model() {
        let store = Arc::new(FlakyStore::default());
        let model = Arc::new(RecordingModel::default());
        let (_thread, handle) = TapestryThread::builder(model.clone(), test_tools())
            .event_store(store.clone())
            .spawn();
        let mut client = Client::new(handle).await;
        client.configure_without_time_context().await;

        store.fail_next_read();
        let events = client.turn("hello", None).await;

        let error = events
            .iter()
            .find(|event| matches!(event.msg, EventMsg::Error(_)))
            .expect("thread should report the failed snapshot read");
        assert!(
            events.iter().all(|event| event.id == error.id),
            "error must retain submission id"
        );
        assert!(matches!(
            &error.msg,
            EventMsg::Error(error) if error.message.contains("read unavailable")
        ));
        assert!(events
            .iter()
            .any(|event| matches!(event.msg, EventMsg::ModelRequestStarted(_))));
        assert!(model.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn failed_reminder_append_does_not_fall_back_to_another_model_request() {
        let store = Arc::new(FlakyStore::default());
        let model = Arc::new(RecordingModel::default());
        let (_thread, handle) = TapestryThread::builder(model.clone(), test_tools())
            .event_store(store.clone())
            .spawn();
        let mut client = Client::new(handle).await;
        client.configure_without_time_context().await;
        client
            .settings(ThreadSettings {
                max_tool_iterations: Some(1),
                ..Default::default()
            })
            .await;

        // The forced-finish reminder is the ninth append in this turn.
        store.fail_once_after(8);
        let events = client.turn("tool:echo", None).await;

        let error = events
            .iter()
            .find(|event| matches!(event.msg, EventMsg::Error(_)))
            .expect("thread should report the failed reminder append");
        assert!(events.iter().all(|event| event.id == error.id));
        assert!(matches!(
            &error.msg,
            EventMsg::Error(error) if error.message.contains("disk full")
        ));
        assert!(events
            .iter()
            .any(|event| matches!(event.msg, EventMsg::ModelRequestStarted(_))));
        assert!(!events
            .iter()
            .any(|event| matches!(event.msg, EventMsg::SystemReminder(_))));
        assert_eq!(model.requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn injected_store_receives_every_event() {
        let store = Arc::new(InMemoryEventStore::new());
        let (_t, handle) = TapestryThread::builder(Arc::new(MockModelClient), test_tools())
            .event_store(store.clone())
            .spawn();
        let mut c = Client::new(handle.clone()).await;
        c.configure().await;
        let events = c.turn("hi", None).await;
        let stored = store.read_after(handle.thread_id(), 0).await.unwrap();
        assert_eq!(stored.last(), events.last());
    }

    #[tokio::test]
    async fn live_history_equals_refolded_log() {
        let store = Arc::new(InMemoryEventStore::new());
        let model = Arc::new(RecordingModel::default());
        let clock = Arc::new(FixedClock::new("2026-11-04", "UTC"));
        let (_t, handle) = TapestryThread::builder(model.clone(), test_tools())
            .event_store(store.clone())
            .clock(clock)
            .spawn();
        let mut c = Client::new(handle.clone()).await;
        c.configure().await;
        c.turn("tool:echo first", None).await;
        c.turn("plain", Some("alice")).await;

        let log = store.read_after(handle.thread_id(), 0).await.unwrap();
        let folded = ConversationProjection::fold(&log).messages();
        model.requests.lock().unwrap().clear();
        c.turn("next", None).await;
        let captured = model.requests.lock().unwrap()[0].clone();
        let request = request_history(&captured);
        assert_eq!(&request[..folded.len()], folded.as_slice());
        assert_eq!(request.len(), folded.len() + 1);
        assert_eq!(&captured.events[..log.len()], log.as_slice());
        assert!(matches!(
            captured.events.last().map(|event| &event.msg),
            Some(EventMsg::ModelRequestStarted(_))
        ));
        assert_eq!(captured.events.len(), log.len() + 3);
        assert!(
            store.read_after(handle.thread_id(), 0).await.unwrap().len() > captured.events.len()
        );
    }

    #[tokio::test]
    async fn consumed_reminders_remain_in_the_log_but_not_in_later_messages() {
        let store = Arc::new(InMemoryEventStore::new());
        let model = Arc::new(RecordingModel::default());
        let (_thread, handle) = TapestryThread::builder(model.clone(), test_tools())
            .event_store(store.clone())
            .spawn();
        let mut client = Client::new(handle).await;
        client.configure_without_time_context().await;
        client
            .settings(ThreadSettings {
                max_tool_iterations: Some(1),
                ..Default::default()
            })
            .await;

        client.turn("tool:echo", None).await;
        client.turn("plain", None).await;

        let next = model.requests.lock().unwrap()[2].clone();
        assert_eq!(
            next.events
                .iter()
                .filter(|event| matches!(event.msg, EventMsg::SystemReminder(_)))
                .count(),
            1
        );
        let reminder_text =
            render_system_reminder(&tapestry_protocol::SystemReminder::ToolLimitReached);
        assert!(OpenAiFormatter
            .format(&next.events)
            .iter()
            .all(|message| message.content.first_text() != Some(reminder_text.as_str())));

        let stored = store
            .read_after(client.handle.thread_id(), 0)
            .await
            .unwrap();
        assert!(stored
            .iter()
            .any(|event| matches!(event.msg, EventMsg::SystemReminder(_))));
        assert!(!ConversationProjection::fold(&stored)
            .messages()
            .iter()
            .any(|item| matches!(item, ConversationItem::SystemReminder(_))));
    }

    #[tokio::test]
    async fn resumed_thread_continues_history_and_log_position() {
        let store = Arc::new(InMemoryEventStore::new());
        let (thread, handle) = TapestryThread::builder(Arc::new(HistoryCountModel), test_tools())
            .event_store(store.clone())
            .spawn();
        let thread_id = thread.thread_id();
        let mut c = Client::new(handle).await;
        c.configure_without_time_context().await;
        assert_eq!(agent_text(&c.turn("one", None).await), "items=1");
        drop((thread, c));

        store
            .append(
                thread_id,
                NewEvent {
                    id: SubmissionId::new(),
                    turn_id: None,
                    timestamp_ms: 10,
                    msg: EventMsg::SystemReminder(tapestry_protocol::SystemReminderEvent {
                        reminder: tapestry_protocol::SystemReminder::ToolLimitReached,
                    }),
                },
            )
            .await
            .unwrap();
        let last_log_position = store.read_after(thread_id, 0).await.unwrap().len() as u64;
        let resumed_model = Arc::new(RecordingModel::default());
        let (resumed, handle) = TapestryThread::builder(resumed_model.clone(), test_tools())
            .event_store(store.clone())
            .resume(thread_id)
            .await
            .unwrap();
        assert_eq!(resumed.thread_id(), thread_id);
        let mut c = Client::after(handle, last_log_position).await;
        c.configure_without_time_context().await;
        let events = c.turn("two", None).await;
        assert_eq!(agent_text(&events), "Echo: two");
        let request = resumed_model.requests.lock().unwrap()[0].clone();
        assert!(request
            .events
            .iter()
            .any(|event| matches!(event.msg, EventMsg::SystemReminder(_))));
        let reminder_text =
            render_system_reminder(&tapestry_protocol::SystemReminder::ToolLimitReached);
        assert!(OpenAiFormatter
            .format(&request.events)
            .iter()
            .all(|message| message.content.first_text() != Some(reminder_text.as_str())));
        let log = store.read_after(thread_id, 0).await.unwrap();
        assert!(!ConversationProjection::fold(&log)
            .messages()
            .iter()
            .any(|item| matches!(item, ConversationItem::SystemReminder(_))));
        let positions: Vec<_> = log.iter().map(|e| e.log_position).collect();
        assert_eq!(positions, (1..=log.len() as u64).collect::<Vec<_>>());
        // The resumed thread's first event directly follows the old log.
        assert!(matches!(
            log[last_log_position as usize].msg,
            EventMsg::SessionConfigured(_)
        ));
        assert_eq!(events.last().unwrap().log_position, log.len() as u64);
    }

    #[tokio::test]
    async fn resumed_thread_is_unconfigured() {
        let store = Arc::new(InMemoryEventStore::new());
        let (thread, handle) = TapestryThread::builder(Arc::new(MockModelClient), test_tools())
            .event_store(store.clone())
            .spawn();
        let mut c = Client::new(handle).await;
        c.configure().await;
        c.turn("hi", None).await;
        let thread_id = thread.thread_id();
        drop((thread, c));

        let (_t, handle) = TapestryThread::builder(Arc::new(MockModelClient), test_tools())
            .event_store(store)
            .resume(thread_id)
            .await
            .unwrap();
        let mut c = Client::new(handle).await;
        let events = c.turn("again", None).await;
        assert!(matches!(events.last().unwrap().msg, EventMsg::Error(_)));
    }
}
