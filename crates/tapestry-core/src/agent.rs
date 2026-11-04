use std::sync::Arc;
use std::time::Instant;

use tapestry_protocol::{
    AgentMessageEvent, Event, EventMsg, ModelRequestCompletedEvent, ModelRequestStartedEvent,
    SubmissionId, TokenCountEvent, ToolCallBeginEvent, ToolCallEndEvent, ToolCallStatus,
    TurnCompleteEvent, TurnId, TurnStartedEvent, WarningEvent,
};
use tokio::sync::mpsc;

use crate::clock::{Clock, SystemClock};
use crate::config::ThreadConfig;
use crate::context::{next_context_entry, ContextState};
use crate::error::CoreError;
use crate::history::{HistoryItem, ToolCall};
use crate::model::{ModelClient, ModelRequest};
use crate::tool::{ToolContext, ToolRegistry};
use crate::turn::{ToolOutcome, TurnMachine, TurnStep};

/// User-side input that starts a turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserInput {
    pub content: String,
    pub name: Option<String>,
}

impl UserInput {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            name: None,
        }
    }

    pub fn named(name: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            name: Some(name.into()),
        }
    }
}

/// Drives a [`TurnMachine`]: performs model/tool IO, timing, and event emission.
pub struct AgentLoop {
    model: Arc<dyn ModelClient>,
    tools: Arc<ToolRegistry>,
    clock: Arc<dyn Clock>,
}

struct Emitter<'a> {
    submission_id: SubmissionId,
    tx: &'a mpsc::Sender<Event>,
}

impl Emitter<'_> {
    async fn emit(&self, msg: EventMsg) -> Result<(), CoreError> {
        self.tx
            .send(Event::new(self.submission_id, msg))
            .await
            .map_err(|_| CoreError::ChannelClosed)
    }
}

impl AgentLoop {
    pub fn new(model: Arc<dyn ModelClient>, tools: Arc<ToolRegistry>) -> Self {
        Self::with_clock(model, tools, Arc::new(SystemClock))
    }

    pub fn with_clock(
        model: Arc<dyn ModelClient>,
        tools: Arc<ToolRegistry>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            model,
            tools,
            clock,
        }
    }

    /// Run one turn. A time-context entry (when due) and the user message are
    /// appended to `history`, which keeps every item produced by the turn (and
    /// stays valid even on error). `context` tracks what the model was told.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_turn(
        &self,
        submission_id: SubmissionId,
        turn_id: TurnId,
        input: UserInput,
        history: &mut Vec<HistoryItem>,
        context: &mut ContextState,
        config: &ThreadConfig,
        event_tx: &mpsc::Sender<Event>,
    ) -> Result<(), CoreError> {
        let user_message = input.content.clone();
        if let Some(entry) =
            next_context_entry(context, config.include_time_context, self.clock.as_ref())
        {
            history.push(HistoryItem::user(None, entry));
        }
        history.push(HistoryItem::user(input.name, input.content));
        let out = Emitter {
            submission_id,
            tx: event_tx,
        };
        let mut machine = TurnMachine::new(history, config.max_tool_iterations);

        let result = self
            .drive(&mut machine, turn_id, &user_message, config, &out)
            .await;
        if result.is_err() {
            machine.repair_history();
        }
        result
    }

    async fn drive(
        &self,
        machine: &mut TurnMachine<'_>,
        turn_id: TurnId,
        user_message: &str,
        config: &ThreadConfig,
        out: &Emitter<'_>,
    ) -> Result<(), CoreError> {
        out.emit(EventMsg::TurnStarted(TurnStartedEvent { turn_id }))
            .await?;

        let mut total_input = 0u32;
        let mut total_output = 0u32;
        let ctx = ToolContext {
            turn_user_message: user_message.to_string(),
        };

        let (finish_reason, last_agent_message) = loop {
            match machine.next_step() {
                TurnStep::CallModel {
                    iteration,
                    allow_tools,
                    reminder,
                } => {
                    out.emit(EventMsg::ModelRequestStarted(ModelRequestStartedEvent {
                        turn_id,
                        iteration,
                        allow_tools,
                    }))
                    .await?;

                    let mut history = machine.history().to_vec();
                    if let Some(reminder) = reminder {
                        history.push(HistoryItem::user(None, reminder));
                    }
                    let response = self
                        .model
                        .complete(ModelRequest {
                            system_prompt: config.effective_system_prompt(),
                            history,
                            tools: if allow_tools {
                                self.tools.specs()
                            } else {
                                Vec::new()
                            },
                            iteration,
                        })
                        .await?;
                    total_input += response.input_tokens;
                    total_output += response.output_tokens;

                    if let Some(content) = &response.content {
                        out.emit(EventMsg::AgentMessage(AgentMessageEvent {
                            turn_id,
                            content: content.clone(),
                        }))
                        .await?;
                    }

                    let effect = machine.on_model_output(response.content, response.tool_calls)?;
                    if effect.ignored_tool_calls > 0 {
                        out.emit(EventMsg::Warning(WarningEvent {
                            turn_id: Some(turn_id),
                            message: format!(
                                "ignored {} tool call(s) requested after the iteration limit",
                                effect.ignored_tool_calls
                            ),
                        }))
                        .await?;
                    }

                    if let TurnStep::RunTools { calls } = machine.next_step() {
                        let mut outcomes = Vec::with_capacity(calls.len());
                        for call in calls {
                            outcomes.push(self.run_tool(turn_id, call, &ctx, out).await?);
                        }
                        machine.on_tool_outcomes(outcomes)?;
                    }

                    out.emit(EventMsg::ModelRequestCompleted(
                        ModelRequestCompletedEvent {
                            turn_id,
                            iteration,
                            input_tokens: response.input_tokens,
                            output_tokens: response.output_tokens,
                        },
                    ))
                    .await?;
                }
                TurnStep::RunTools { .. } => {
                    return Err(CoreError::Internal("unexpected pending tool calls".into()));
                }
                TurnStep::Done {
                    finish,
                    last_agent_message,
                } => break (finish, last_agent_message),
            }
        };

        out.emit(EventMsg::TokenCount(TokenCountEvent {
            turn_id,
            input_tokens: total_input,
            output_tokens: total_output,
            total_tokens: total_input + total_output,
        }))
        .await?;
        out.emit(EventMsg::TurnComplete(TurnCompleteEvent {
            turn_id,
            last_agent_message,
            finish_reason,
        }))
        .await
    }

    async fn run_tool(
        &self,
        turn_id: TurnId,
        call: ToolCall,
        ctx: &ToolContext,
        out: &Emitter<'_>,
    ) -> Result<ToolOutcome, CoreError> {
        out.emit(EventMsg::ToolCallBegin(ToolCallBeginEvent {
            turn_id,
            tool_call_id: call.id,
            tool_name: call.name.clone(),
            arguments: call.arguments.clone(),
        }))
        .await?;

        let started = Instant::now();
        let (output, status) = match self.tools.get(&call.name) {
            Some(tool) => {
                let result = tool.execute(call.arguments.clone(), ctx).await;
                let status = if result.success {
                    ToolCallStatus::Completed
                } else {
                    ToolCallStatus::Failed
                };
                (result.output, status)
            }
            None => (
                format!("tool `{}` not found", call.name),
                ToolCallStatus::NotFound,
            ),
        };
        let duration_ms = started.elapsed().as_millis() as u64;

        out.emit(EventMsg::ToolCallEnd(ToolCallEndEvent {
            turn_id,
            tool_call_id: call.id,
            tool_name: call.name.clone(),
            output: output.clone(),
            success: status == ToolCallStatus::Completed,
            status,
            duration_ms: Some(duration_ms),
        }))
        .await?;

        Ok(ToolOutcome {
            call_id: call.id,
            tool_name: call.name,
            output,
            status,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use serde_json::Value;
    use tapestry_protocol::{FinishReason, SubmissionId, TurnId};

    use super::*;
    use crate::clock::FixedClock;
    use crate::model::{MockModelClient, ModelResponse};
    use crate::tool::{EchoTool, Tool, ToolResult, ToolSpec};

    /// Replays scripted responses and records every request.
    struct ScriptedModel {
        script: Mutex<VecDeque<Result<ModelResponse, CoreError>>>,
        requests: Mutex<Vec<ModelRequest>>,
    }

    impl ScriptedModel {
        fn new(script: Vec<Result<ModelResponse, CoreError>>) -> Arc<Self> {
            Arc::new(Self {
                script: Mutex::new(script.into()),
                requests: Mutex::new(Vec::new()),
            })
        }
    }

    #[async_trait]
    impl ModelClient for ScriptedModel {
        async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, CoreError> {
            self.requests.lock().unwrap().push(request);
            self.script
                .lock()
                .unwrap()
                .pop_front()
                .expect("script exhausted")
        }
    }

    fn resp(content: Option<&str>, calls: Vec<ToolCall>) -> Result<ModelResponse, CoreError> {
        Ok(ModelResponse {
            content: content.map(str::to_string),
            tool_calls: calls,
            input_tokens: 1,
            output_tokens: 2,
        })
    }

    struct FailingTool;

    #[async_trait]
    impl Tool for FailingTool {
        fn name(&self) -> &str {
            "fail"
        }
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "fail".into(),
                description: "always fails".into(),
                parameters: serde_json::json!({"type": "object"}),
            }
        }
        async fn execute(&self, _a: Value, _c: &ToolContext) -> ToolResult {
            ToolResult::failure("nope")
        }
    }

    fn registry() -> Arc<ToolRegistry> {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        registry.register(Arc::new(FailingTool));
        Arc::new(registry)
    }

    async fn run(
        model: Arc<dyn ModelClient>,
        message: &str,
        history: &mut Vec<HistoryItem>,
        config: &ThreadConfig,
        capacity: usize,
    ) -> (Result<(), CoreError>, Vec<Event>, SubmissionId) {
        let loop_ = AgentLoop::new(model, registry());
        let (tx, mut rx) = mpsc::channel(capacity);
        let sid = SubmissionId::new();
        // Time context has dedicated tests; keep the rest of the history exact.
        let config = ThreadConfig {
            include_time_context: false,
            ..config.clone()
        };
        let result = loop_
            .run_turn(
                sid,
                TurnId::new(),
                UserInput::new(message),
                history,
                &mut ContextState::default(),
                &config,
                &tx,
            )
            .await;
        drop(tx);
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        (result, events, sid)
    }

    fn no_time_context() -> ThreadConfig {
        ThreadConfig {
            include_time_context: false,
            ..Default::default()
        }
    }

    fn kinds(events: &[Event]) -> Vec<String> {
        events
            .iter()
            .map(|e| match &e.msg {
                EventMsg::TurnStarted(_) => "turn_started".into(),
                EventMsg::ModelRequestStarted(m) => format!("req_start:{}", m.iteration),
                EventMsg::ModelRequestCompleted(m) => format!("req_done:{}", m.iteration),
                EventMsg::AgentMessage(_) => "agent_message".into(),
                EventMsg::ToolCallBegin(_) => "tool_begin".into(),
                EventMsg::ToolCallEnd(t) => format!("tool_end:{:?}", t.status),
                EventMsg::Warning(_) => "warning".into(),
                EventMsg::TokenCount(_) => "token_count".into(),
                EventMsg::TurnComplete(_) => "turn_complete".into(),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[tokio::test]
    async fn direct_answer_event_order() {
        let mut history = Vec::new();
        let (result, events, sid) = run(
            Arc::new(MockModelClient),
            "hello",
            &mut history,
            &ThreadConfig::default(),
            32,
        )
        .await;
        result.unwrap();
        assert_eq!(
            kinds(&events),
            [
                "turn_started",
                "req_start:0",
                "agent_message",
                "req_done:0",
                "token_count",
                "turn_complete"
            ]
        );
        assert!(events.iter().all(|e| e.id == sid));
        match &events.last().unwrap().msg {
            EventMsg::TurnComplete(c) => {
                assert_eq!(c.finish_reason, FinishReason::Completed);
                assert_eq!(c.last_agent_message.as_deref(), Some("Echo: hello"));
            }
            _ => panic!(),
        }
    }

    #[tokio::test]
    async fn tool_turn_event_order_and_history() {
        let mut history = Vec::new();
        let (result, events, _) = run(
            Arc::new(MockModelClient),
            "please tool:echo now",
            &mut history,
            &ThreadConfig::default(),
            32,
        )
        .await;
        result.unwrap();
        assert_eq!(
            kinds(&events),
            [
                "turn_started",
                "req_start:0",
                "tool_begin",
                "tool_end:Completed",
                "req_done:0",
                "req_start:1",
                "agent_message",
                "req_done:1",
                "token_count",
                "turn_complete"
            ]
        );
        let (begin, end) = events.iter().fold((None, None), |acc, e| match &e.msg {
            EventMsg::ToolCallBegin(b) => (Some(b.tool_call_id), acc.1),
            EventMsg::ToolCallEnd(t) => (acc.0, Some((t.tool_call_id, t.duration_ms))),
            _ => acc,
        });
        assert_eq!(begin, end.map(|e| e.0));
        assert!(end.unwrap().1.is_some());

        assert!(matches!(history[0], HistoryItem::User { .. }));
        assert!(
            matches!(&history[1], HistoryItem::Assistant { tool_calls, .. } if tool_calls.len() == 1)
        );
        assert!(matches!(history[2], HistoryItem::ToolResult { .. }));
        assert!(matches!(
            &history[3],
            HistoryItem::Assistant {
                content: Some(_),
                ..
            }
        ));
    }

    #[tokio::test]
    async fn unknown_and_failing_tools_are_fed_back() {
        let ghost = ToolCall::new("ghost", Value::Null);
        let fail = ToolCall::new("fail", Value::Null);
        let model = ScriptedModel::new(vec![
            resp(Some("thinking"), vec![ghost.clone(), fail.clone()]),
            resp(Some("recovered"), vec![]),
        ]);
        let mut history = Vec::new();
        let (result, events, _) = run(
            model.clone(),
            "go",
            &mut history,
            &ThreadConfig::default(),
            32,
        )
        .await;
        result.unwrap();
        assert_eq!(
            kinds(&events),
            [
                "turn_started",
                "req_start:0",
                "agent_message",
                "tool_begin",
                "tool_end:NotFound",
                "tool_begin",
                "tool_end:Failed",
                "req_done:0",
                "req_start:1",
                "agent_message",
                "req_done:1",
                "token_count",
                "turn_complete"
            ]
        );
        let ends: Vec<_> = events
            .iter()
            .filter_map(|e| match &e.msg {
                EventMsg::ToolCallEnd(t) => Some((t.tool_call_id, t.success, t.output.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(ends[0].0, ghost.id);
        assert!(!ends[0].1 && ends[0].2.contains("ghost"));
        assert_eq!(ends[1].0, fail.id);
        assert_eq!(ends[1].2, "nope");

        let requests = model.requests.lock().unwrap();
        assert_eq!(requests[1].history.len(), 4);
        assert_eq!(requests[0].tools.len(), 2);
    }

    #[tokio::test]
    async fn exceeding_iterations_forces_tool_free_finish() {
        let echo = || ToolCall::new("echo", serde_json::json!({"input": "x"}));
        let model = ScriptedModel::new(vec![
            resp(None, vec![echo()]),
            resp(None, vec![echo()]),
            resp(Some("summary"), vec![]),
        ]);
        let config = ThreadConfig {
            max_tool_iterations: 2,
            ..Default::default()
        };
        let mut history = Vec::new();
        let (result, events, _) = run(model.clone(), "go", &mut history, &config, 64).await;
        result.unwrap();

        assert_eq!(
            kinds(&events).last_chunk::<2>().unwrap(),
            &["token_count".to_string(), "turn_complete".to_string()]
        );
        let started: Vec<_> = events
            .iter()
            .filter_map(|e| match &e.msg {
                EventMsg::ModelRequestStarted(m) => Some((m.iteration, m.allow_tools)),
                _ => None,
            })
            .collect();
        assert_eq!(started, [(0, true), (1, true), (2, false)]);
        match &events.last().unwrap().msg {
            EventMsg::TurnComplete(c) => {
                assert_eq!(c.finish_reason, FinishReason::ExceedMaxIters);
                assert_eq!(c.last_agent_message.as_deref(), Some("summary"));
            }
            _ => panic!(),
        }
        assert!(!events.iter().any(|e| matches!(e.msg, EventMsg::Error(_))));

        let requests = model.requests.lock().unwrap();
        assert!(requests[2].tools.is_empty());
        assert!(matches!(
            requests[2].history.last(),
            Some(HistoryItem::User { name: None, .. })
        ));
        assert_eq!(requests[2].history.len(), 6);
        // reminder is not persisted
        assert_eq!(history.len(), 6);
        assert_eq!(
            history
                .iter()
                .filter(|i| matches!(i, HistoryItem::User { .. }))
                .count(),
            1
        );
        assert!(
            matches!(&history[5], HistoryItem::Assistant { content: Some(c), .. } if c == "summary")
        );
    }

    #[tokio::test]
    async fn forced_finish_tool_calls_warn_and_are_ignored() {
        let model = ScriptedModel::new(vec![
            resp(None, vec![ToolCall::new("echo", Value::Null)]),
            resp(Some("still"), vec![ToolCall::new("echo", Value::Null)]),
        ]);
        let config = ThreadConfig {
            max_tool_iterations: 1,
            ..Default::default()
        };
        let mut history = Vec::new();
        let (result, events, _) = run(model, "go", &mut history, &config, 64).await;
        result.unwrap();
        let k = kinds(&events);
        assert_eq!(k.iter().filter(|s| *s == "warning").count(), 1);
        assert_eq!(k.iter().filter(|s| s.starts_with("tool_begin")).count(), 1);
        assert_eq!(k.last().unwrap(), "turn_complete");
        assert!(matches!(
            history.last(),
            Some(HistoryItem::Assistant { tool_calls, .. }) if tool_calls.is_empty()
        ));
    }

    #[tokio::test]
    async fn model_failure_returns_error_and_keeps_history_valid() {
        let model = ScriptedModel::new(vec![
            resp(None, vec![ToolCall::new("echo", Value::Null)]),
            Err(CoreError::Model("down".into())),
        ]);
        let mut history = Vec::new();
        let (result, events, _) =
            run(model, "go", &mut history, &ThreadConfig::default(), 32).await;
        assert!(matches!(result, Err(CoreError::Model(_))));
        assert!(!kinds(&events).contains(&"turn_complete".to_string()));
        // every tool call has a result
        let calls: Vec<_> = history
            .iter()
            .flat_map(|i| match i {
                HistoryItem::Assistant { tool_calls, .. } => {
                    tool_calls.iter().map(|c| c.id).collect()
                }
                _ => vec![],
            })
            .collect();
        for id in calls {
            assert!(history
                .iter()
                .any(|i| matches!(i, HistoryItem::ToolResult { call_id, .. } if *call_id == id)));
        }
    }

    #[tokio::test]
    async fn model_failure_on_tool_less_turn_leaves_user_message() {
        let model = ScriptedModel::new(vec![Err(CoreError::Model("down".into()))]);
        let mut history = Vec::new();
        let (result, _, _) = run(model, "go", &mut history, &ThreadConfig::default(), 32).await;
        assert!(result.is_err());
        assert_eq!(history.len(), 1);
    }

    #[tokio::test]
    async fn closed_channel_terminates_with_channel_closed() {
        let loop_ = AgentLoop::new(Arc::new(MockModelClient), registry());
        let (tx, rx) = mpsc::channel(4);
        drop(rx);
        let mut history = Vec::new();
        let result = loop_
            .run_turn(
                SubmissionId::new(),
                TurnId::new(),
                UserInput::new("tool:echo"),
                &mut history,
                &mut ContextState::default(),
                &no_time_context(),
                &tx,
            )
            .await;
        assert!(matches!(result, Err(CoreError::ChannelClosed)));
    }

    #[tokio::test]
    async fn closed_channel_mid_tool_run_repairs_history() {
        let loop_ = AgentLoop::new(
            ScriptedModel::new(vec![resp(
                None,
                vec![
                    ToolCall::new("echo", Value::Null),
                    ToolCall::new("echo", Value::Null),
                ],
            )]),
            registry(),
        );
        let (tx, mut rx) = mpsc::channel(3);
        // Read up to ToolCallBegin of the first call, then close the receiver.
        let consumer = tokio::spawn(async move {
            for _ in 0..3 {
                rx.recv().await;
            }
        });
        let mut history = Vec::new();
        let result = loop_
            .run_turn(
                SubmissionId::new(),
                TurnId::new(),
                UserInput::new("go"),
                &mut history,
                &mut ContextState::default(),
                &no_time_context(),
                &tx,
            )
            .await;
        consumer.await.unwrap();
        assert!(matches!(result, Err(CoreError::ChannelClosed)));
        let results = history
            .iter()
            .filter(|i| matches!(i, HistoryItem::ToolResult { .. }))
            .count();
        assert_eq!(results, 2);
    }

    #[tokio::test]
    async fn request_carries_system_prompt_with_agent_name() {
        let model = ScriptedModel::new(vec![resp(Some("ok"), vec![])]);
        let config = ThreadConfig {
            system_prompt: Some("Base.".into()),
            agent_name: Some("planner".into()),
            ..Default::default()
        };
        let mut history = Vec::new();
        let (result, _, _) = run(model.clone(), "go", &mut history, &config, 32).await;
        result.unwrap();
        let prompt = model.requests.lock().unwrap()[0]
            .system_prompt
            .clone()
            .unwrap();
        assert!(prompt.starts_with("Base.") && prompt.contains("planner"));
    }

    struct TimeHarness {
        agent: AgentLoop,
        model: Arc<ScriptedModel>,
        clock: Arc<FixedClock>,
        history: Vec<HistoryItem>,
        context: ContextState,
        config: ThreadConfig,
    }

    impl TimeHarness {
        fn new(turns: usize) -> Self {
            let model = ScriptedModel::new((0..turns).map(|_| resp(Some("ok"), vec![])).collect());
            let clock = Arc::new(FixedClock::new("2026-11-04", "Asia/Shanghai"));
            Self {
                agent: AgentLoop::with_clock(model.clone(), registry(), clock.clone()),
                model,
                clock,
                history: Vec::new(),
                context: ContextState::default(),
                config: ThreadConfig::default(),
            }
        }

        async fn turn(&mut self, message: &str) -> (Result<(), CoreError>, Vec<Event>) {
            let (tx, mut rx) = mpsc::channel(64);
            let result = self
                .agent
                .run_turn(
                    SubmissionId::new(),
                    TurnId::new(),
                    UserInput::new(message),
                    &mut self.history,
                    &mut self.context,
                    &self.config,
                    &tx,
                )
                .await;
            drop(tx);
            let mut events = Vec::new();
            while let Some(event) = rx.recv().await {
                events.push(event);
            }
            (result, events)
        }

        fn context_entries(&self) -> Vec<&str> {
            self.history
                .iter()
                .filter_map(|i| match i {
                    HistoryItem::User {
                        name: None,
                        content,
                    } if content.starts_with("<environment_context>") => Some(content.as_str()),
                    _ => None,
                })
                .collect()
        }
    }

    #[tokio::test]
    async fn first_turn_puts_time_context_before_user_message() {
        let mut h = TimeHarness::new(1);
        let (result, _) = h.turn("hello").await;
        result.unwrap();
        match (&h.history[0], &h.history[1]) {
            (
                HistoryItem::User {
                    name: None,
                    content: ctx,
                },
                HistoryItem::User { content: msg, .. },
            ) => {
                assert!(ctx.contains("<current_date>2026-11-04</current_date>"));
                assert!(ctx.contains("<timezone>Asia/Shanghai</timezone>"));
                assert_eq!(msg, "hello");
            }
            other => panic!("unexpected history: {other:?}"),
        }
        let sent = &h.model.requests.lock().unwrap()[0].history;
        assert!(
            matches!(&sent[0], HistoryItem::User { content, .. } if content.contains("environment_context"))
        );
    }

    #[tokio::test]
    async fn same_day_turn_does_not_resend_and_next_day_adds_date_only() {
        let mut h = TimeHarness::new(3);
        h.turn("a").await.0.unwrap();
        h.turn("b").await.0.unwrap();
        assert_eq!(h.context_entries().len(), 1);
        h.clock.set("2026-11-05", "Asia/Shanghai");
        h.turn("c").await.0.unwrap();
        let entries = h.context_entries();
        assert_eq!(entries.len(), 2);
        assert!(entries[1].contains("2026-11-05") && !entries[1].contains("timezone"));
    }

    #[tokio::test]
    async fn disabled_from_start_never_reads_clock_or_injects() {
        let mut h = TimeHarness::new(1);
        h.config.include_time_context = false;
        h.clock.fail();
        let (result, _) = h.turn("hi").await;
        result.unwrap();
        assert_eq!(h.history.len(), 2);
        assert!(h.context_entries().is_empty());
    }

    #[tokio::test]
    async fn disabling_midway_sends_unavailable_once() {
        let mut h = TimeHarness::new(3);
        h.turn("a").await.0.unwrap();
        h.config.include_time_context = false;
        h.turn("b").await.0.unwrap();
        h.turn("c").await.0.unwrap();
        let entries = h.context_entries();
        assert_eq!(entries.len(), 2);
        assert!(entries[1].contains(r#"status="unavailable""#));
    }

    #[tokio::test]
    async fn clock_failure_does_not_fail_turn_and_recovers_fully() {
        let mut h = TimeHarness::new(3);
        h.clock.fail();
        let (result, events) = h.turn("a").await;
        result.unwrap();
        assert!(!events.iter().any(|e| matches!(e.msg, EventMsg::Error(_))));
        h.turn("b").await.0.unwrap();
        assert_eq!(h.context_entries().len(), 1);
        h.clock.set("2026-11-04", "UTC");
        h.turn("c").await.0.unwrap();
        let entries = h.context_entries();
        assert_eq!(entries.len(), 2);
        assert!(entries[1].contains("current_date") && entries[1].contains("timezone"));
    }

    #[tokio::test]
    async fn failed_turn_keeps_context_entry_and_valid_history() {
        let model = ScriptedModel::new(vec![
            Err(CoreError::Model("down".into())),
            resp(Some("ok"), vec![]),
        ]);
        let clock = Arc::new(FixedClock::new("2026-11-04", "UTC"));
        let agent = AgentLoop::with_clock(model, registry(), clock);
        let (tx, _rx) = mpsc::channel(64);
        let mut history = Vec::new();
        let mut ctx = ContextState::default();
        let config = ThreadConfig::default();
        let first = agent
            .run_turn(
                SubmissionId::new(),
                TurnId::new(),
                UserInput::new("a"),
                &mut history,
                &mut ctx,
                &config,
                &tx,
            )
            .await;
        assert!(first.is_err());
        assert_eq!(history.len(), 2);
        agent
            .run_turn(
                SubmissionId::new(),
                TurnId::new(),
                UserInput::new("b"),
                &mut history,
                &mut ctx,
                &config,
                &tx,
            )
            .await
            .unwrap();
        let count = history
            .iter()
            .filter(|i| matches!(i, HistoryItem::User { content, .. } if content.contains("environment_context")))
            .count();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn time_context_keeps_event_order_and_adds_no_events() {
        let mut h = TimeHarness::new(1);
        let (result, events) = h.turn("hello").await;
        result.unwrap();
        assert_eq!(
            kinds(&events),
            vec![
                "turn_started",
                "req_start:0",
                "agent_message",
                "req_done:0",
                "token_count",
                "turn_complete"
            ]
        );
    }
}
