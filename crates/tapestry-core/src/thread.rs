use std::sync::Arc;

use tokio::sync::{mpsc, Mutex};
use tracing::{debug, warn};

use tapestry_protocol::{
    ErrorEvent, Event, EventMsg, Op, SessionConfiguredEvent, Submission, SubmissionId, ThreadId,
    TurnAbortReason, TurnAbortedEvent, TurnId,
};

use crate::agent::AgentLoop;
use crate::clock::{Clock, SystemClock};
use crate::config::ThreadConfig;
use crate::error::CoreError;
use crate::model::ModelClient;
use crate::tool::ToolRegistry;
use tapestry_protocol::HistoryItem;

const DEFAULT_CHANNEL_CAPACITY: usize = 64;

/// Per-thread session state owned by the worker task.
#[derive(Default)]
struct Session {
    config: ThreadConfig,
    configured: bool,
    active_turn: Option<TurnId>,
    /// Kept across turns; in-memory only and unbounded.
    history: Vec<HistoryItem>,
}

impl Session {
    /// The only way to rewrite history wholesale (compaction, rollback, restore).
    /// The time-context baseline is derived from history, so nothing else resets.
    #[allow(dead_code)]
    fn replace_history(&mut self, history: Vec<HistoryItem>) {
        self.history = history;
    }
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

/// Handle for submitting operations and receiving events from a thread.
#[derive(Clone)]
pub struct ThreadHandle {
    submission_tx: mpsc::Sender<Submission>,
    event_rx: Arc<Mutex<mpsc::Receiver<Event>>>,
}

impl ThreadHandle {
    pub async fn submit(&self, op: Op) -> Result<SubmissionId, CoreError> {
        let submission = Submission::new(op);
        let id = submission.id;
        self.submission_tx
            .send(submission)
            .await
            .map_err(|_| CoreError::ChannelClosed)?;
        Ok(id)
    }

    pub async fn next_event(&self) -> Option<Event> {
        self.event_rx.lock().await.recv().await
    }

    pub async fn collect_events_for(&self, submission_id: SubmissionId) -> Vec<Event> {
        let mut events = Vec::new();
        while let Some(event) = self.next_event().await {
            let done = matches!(
                event.msg,
                EventMsg::TurnComplete(_)
                    | EventMsg::TurnAborted(_)
                    | EventMsg::Error(_)
                    | EventMsg::SessionConfigured(_)
            );
            let same = event.id == submission_id;
            events.push(event);
            if same && done {
                break;
            }
        }
        events
    }
}

/// A long-lived agent thread with SQ/EQ channels.
pub struct TapestryThread {
    thread_id: ThreadId,
    handle: ThreadHandle,
}

impl TapestryThread {
    pub fn spawn(model: Arc<dyn ModelClient>, tools: Arc<ToolRegistry>) -> (Self, ThreadHandle) {
        Self::spawn_with_clock(model, tools, Arc::new(SystemClock))
    }

    pub fn spawn_with_clock(
        model: Arc<dyn ModelClient>,
        tools: Arc<ToolRegistry>,
        clock: Arc<dyn Clock>,
    ) -> (Self, ThreadHandle) {
        let thread_id = ThreadId::new();
        let (submission_tx, submission_rx) = mpsc::channel(DEFAULT_CHANNEL_CAPACITY);
        let (event_tx, event_rx) = mpsc::channel(DEFAULT_CHANNEL_CAPACITY);

        let handle = ThreadHandle {
            submission_tx,
            event_rx: Arc::new(Mutex::new(event_rx)),
        };

        let worker_handle = handle.clone();
        tokio::spawn(async move {
            if let Err(err) =
                run_thread_loop(thread_id, submission_rx, event_tx, model, tools, clock).await
            {
                warn!(?err, "thread loop exited with error");
            }
        });

        let thread = Self {
            thread_id,
            handle: worker_handle.clone(),
        };

        (thread, handle)
    }

    pub fn thread_id(&self) -> ThreadId {
        self.thread_id
    }

    pub fn handle(&self) -> ThreadHandle {
        self.handle.clone()
    }
}

async fn run_thread_loop(
    thread_id: ThreadId,
    mut submission_rx: mpsc::Receiver<Submission>,
    event_tx: mpsc::Sender<Event>,
    model: Arc<dyn ModelClient>,
    tools: Arc<ToolRegistry>,
    clock: Arc<dyn Clock>,
) -> Result<(), CoreError> {
    let mut session = Session::default();
    let agent = AgentLoop::with_clock(model, tools, clock);

    while let Some(submission) = submission_rx.recv().await {
        debug!(submission_id = %submission.id, "processing submission");

        let submission_id = submission.id;
        let result =
            process_submission(&agent, thread_id, &mut session, submission, &event_tx).await;

        if let Err(err) = result {
            let _ = event_tx
                .send(Event::new(
                    submission_id,
                    EventMsg::Error(ErrorEvent {
                        message: err.to_string(),
                    }),
                ))
                .await;
        }
    }

    Ok(())
}

async fn process_submission(
    agent: &AgentLoop,
    thread_id: ThreadId,
    session: &mut Session,
    submission: Submission,
    event_tx: &mpsc::Sender<Event>,
) -> Result<(), CoreError> {
    match submission.op {
        Op::Configure {
            model,
            system_prompt,
        } => {
            session.config.model = model.clone();
            session.config.system_prompt = system_prompt;
            session.configured = true;
            event_tx
                .send(Event::new(
                    submission.id,
                    EventMsg::SessionConfigured(SessionConfiguredEvent { thread_id, model }),
                ))
                .await
                .map_err(|_| CoreError::ChannelClosed)?;
        }
        Op::ThreadSettings { settings } => {
            session.config.apply_settings(&settings);
            if !session.configured {
                session.configured = true;
                event_tx
                    .send(Event::new(
                        submission.id,
                        EventMsg::SessionConfigured(SessionConfiguredEvent {
                            thread_id,
                            model: session.config.model.clone(),
                        }),
                    ))
                    .await
                    .map_err(|_| CoreError::ChannelClosed)?;
            }
        }
        Op::Interrupt => {
            let aborted = session.active_turn.take();
            event_tx
                .send(Event::new(
                    submission.id,
                    EventMsg::TurnAborted(TurnAbortedEvent {
                        turn_id: aborted,
                        reason: TurnAbortReason::Interrupted,
                        message: Some("interrupted by client".into()),
                    }),
                ))
                .await
                .map_err(|_| CoreError::ChannelClosed)?;
        }
        Op::StartTurn {
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

            let run_result = agent
                .run_turn(
                    submission.id,
                    turn_id,
                    content,
                    name,
                    &mut session.history,
                    &session.config,
                    event_tx,
                )
                .await;

            session.active_turn.take();
            run_result?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tapestry_protocol::EventMsg;

    use super::*;
    use crate::model::MockModelClient;
    use crate::tool::{EchoTool, ToolRegistry};

    fn test_tools() -> Arc<ToolRegistry> {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        Arc::new(registry)
    }

    #[tokio::test]
    async fn thread_configure_and_turn() {
        let (_thread, handle) = TapestryThread::spawn(Arc::new(MockModelClient), test_tools());

        let config_id = handle
            .submit(Op::Configure {
                model: "mock-model".into(),
                system_prompt: Some("test".into()),
            })
            .await
            .unwrap();

        let config_events = handle.collect_events_for(config_id).await;
        assert!(matches!(
            config_events.last().map(|e| &e.msg),
            Some(EventMsg::SessionConfigured(_))
        ));

        let turn_id = handle
            .submit(Op::StartTurn {
                content: "hello".into(),
                name: None,
                turn_id: None,
            })
            .await
            .unwrap();

        let turn_events = handle.collect_events_for(turn_id).await;
        assert!(turn_events
            .iter()
            .any(|e| matches!(e.msg, EventMsg::AgentMessage(_))));
        assert!(turn_events
            .iter()
            .any(|e| matches!(e.msg, EventMsg::TurnComplete(_))));
    }

    #[tokio::test]
    async fn thread_rejects_turn_before_configure() {
        let (_thread, handle) = TapestryThread::spawn(Arc::new(MockModelClient), test_tools());

        let turn_id = handle
            .submit(Op::StartTurn {
                content: "hello".into(),
                name: None,
                turn_id: None,
            })
            .await
            .unwrap();

        // Give the worker a moment to emit the error event.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let events = handle.collect_events_for(turn_id).await;
        assert!(events.iter().any(|e| matches!(e.msg, EventMsg::Error(_))));
    }

    async fn configure(handle: &ThreadHandle) {
        let id = handle
            .submit(Op::Configure {
                model: "mock-model".into(),
                system_prompt: None,
            })
            .await
            .unwrap();
        handle.collect_events_for(id).await;
    }

    async fn configure_without_time_context(handle: &ThreadHandle) {
        configure(handle).await;
        handle
            .submit(Op::ThreadSettings {
                settings: tapestry_protocol::ThreadSettings {
                    include_time_context: Some(false),
                    ..Default::default()
                },
            })
            .await
            .unwrap();
    }

    async fn turn(handle: &ThreadHandle, msg: &str, user_name: Option<&str>) -> Vec<Event> {
        let id = handle
            .submit(Op::StartTurn {
                content: msg.into(),
                name: user_name.map(str::to_string),
                turn_id: None,
            })
            .await
            .unwrap();
        handle.collect_events_for(id).await
    }

    fn agent_text(events: &[Event]) -> String {
        events
            .iter()
            .filter_map(|e| match &e.msg {
                EventMsg::AgentMessage(m) => Some(m.content.clone()),
                _ => None,
            })
            .collect()
    }

    /// Echoes the number of history items it sees.
    struct HistoryCountModel;

    #[async_trait::async_trait]
    impl ModelClient for HistoryCountModel {
        async fn complete(
            &self,
            request: tapestry_protocol::ModelRequest,
        ) -> Result<tapestry_protocol::ModelResponse, CoreError> {
            Ok(tapestry_protocol::ModelResponse {
                content: Some(format!("items={}", request.history.len())),
                tool_calls: vec![],
                usage: Default::default(),
            })
        }
    }

    #[tokio::test]
    async fn history_persists_across_turns() {
        let (_t, handle) = TapestryThread::spawn(Arc::new(HistoryCountModel), test_tools());
        configure_without_time_context(&handle).await;
        assert_eq!(agent_text(&turn(&handle, "one", None).await), "items=1");
        // user, assistant, user
        assert_eq!(agent_text(&turn(&handle, "two", None).await), "items=3");
    }

    #[tokio::test]
    async fn named_user_is_rendered_for_model() {
        let (_t, handle) = TapestryThread::spawn(Arc::new(MockModelClient), test_tools());
        configure(&handle).await;
        let events = turn(&handle, "hi", Some("alice")).await;
        assert_eq!(agent_text(&events), "Echo: alice: hi");
    }

    #[tokio::test]
    async fn invalid_user_name_errors_and_leaves_history() {
        let (_t, handle) = TapestryThread::spawn(Arc::new(HistoryCountModel), test_tools());
        configure_without_time_context(&handle).await;
        for bad in ["", "   ", "a\nb"] {
            let events = turn(&handle, "hi", Some(bad)).await;
            assert!(
                matches!(events.last().unwrap().msg, EventMsg::Error(_)),
                "{bad:?}"
            );
            assert!(!events
                .iter()
                .any(|e| matches!(e.msg, EventMsg::TurnStarted(_))));
        }
        assert_eq!(agent_text(&turn(&handle, "ok", None).await), "items=1");
    }

    #[tokio::test]
    async fn interrupt_reports_enum_reason() {
        let (_t, handle) = TapestryThread::spawn(Arc::new(MockModelClient), test_tools());
        let id = handle.submit(Op::Interrupt).await.unwrap();
        let events = handle.collect_events_for(id).await;
        match &events.last().unwrap().msg {
            EventMsg::TurnAborted(a) => assert_eq!(a.reason, TurnAbortReason::Interrupted),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn agent_name_setting_updates_system_prompt() {
        struct PromptModel;
        #[async_trait::async_trait]
        impl ModelClient for PromptModel {
            async fn complete(
                &self,
                request: tapestry_protocol::ModelRequest,
            ) -> Result<tapestry_protocol::ModelResponse, CoreError> {
                Ok(tapestry_protocol::ModelResponse {
                    content: request.system_prompt,
                    tool_calls: vec![],
                    usage: Default::default(),
                })
            }
        }

        let (_t, handle) = TapestryThread::spawn(Arc::new(PromptModel), test_tools());
        configure(&handle).await;
        assert_eq!(agent_text(&turn(&handle, "x", None).await), "");

        handle
            .submit(Op::ThreadSettings {
                settings: tapestry_protocol::ThreadSettings {
                    agent_name: Some("planner".into()),
                    ..Default::default()
                },
            })
            .await
            .unwrap();
        let text = agent_text(&turn(&handle, "x", None).await);
        assert!(text.contains("planner"));
    }

    #[tokio::test]
    async fn thread_injects_time_context_once_per_day_and_honors_setting() {
        use crate::clock::FixedClock;

        /// Echoes how many time-context entries the request history holds.
        struct ContextCountModel;
        #[async_trait::async_trait]
        impl ModelClient for ContextCountModel {
            async fn complete(
                &self,
                request: tapestry_protocol::ModelRequest,
            ) -> Result<tapestry_protocol::ModelResponse, CoreError> {
                let n = request
                    .history
                    .iter()
                    .filter(|i| matches!(i, HistoryItem::EnvironmentContext(_)))
                    .count();
                Ok(tapestry_protocol::ModelResponse {
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
        configure(&handle).await;
        assert_eq!(agent_text(&turn(&handle, "a", None).await), "1");
        assert_eq!(agent_text(&turn(&handle, "b", None).await), "1");
        clock.set("2026-11-05", "UTC");
        assert_eq!(agent_text(&turn(&handle, "c", None).await), "2");

        handle
            .submit(Op::ThreadSettings {
                settings: tapestry_protocol::ThreadSettings {
                    include_time_context: Some(false),
                    ..Default::default()
                },
            })
            .await
            .unwrap();
        // Disabling after a send appends one "unavailable" entry.
        assert_eq!(agent_text(&turn(&handle, "d", None).await), "3");
        assert_eq!(agent_text(&turn(&handle, "e", None).await), "3");
    }

    #[test]
    fn replace_history_replaces_without_other_state() {
        let mut session = Session::default();
        session.history.push(HistoryItem::user(None, "old"));
        session.replace_history(vec![HistoryItem::user(None, "new")]);
        assert_eq!(session.history, vec![HistoryItem::user(None, "new")]);
        session.replace_history(Vec::new());
        assert!(session.history.is_empty());
    }
}
