use std::sync::Arc;
use std::time::Instant;

use tapestry_protocol::{
    AssistantMessageDeltaEvent, AssistantMessageStartEvent, EnvironmentContextUpdatedEvent,
    EventMsg, MessageId, ModelRequest, ModelRequestCompletedEvent, ModelRequestStartedEvent,
    ModelResponse, ModelStreamItem, SystemReminderEvent, TokenCountEvent, TokenUsage, ToolCall,
    ToolCallBeginEvent, ToolCallEndEvent, ToolResult, TurnCompleteEvent, TurnId, TurnStartedEvent,
    UserMessageEvent, WarningEvent,
};

use crate::clock::{Clock, SystemClock};
use crate::config::ThreadConfig;
use crate::context::next_context_entry;
use crate::error::CoreError;
use crate::middleware::{HookCtx, Middleware, ToolDecision};
use crate::model::ModelClient;
use crate::recorder::Recorder;
use crate::tool::ToolRegistry;
use crate::turn::{TurnMachine, TurnStep};

use super::Agent;

/// Drives a [`TurnMachine`]: performs model/tool IO and timing, and records
/// every step as an event.
pub struct AgentLoop {
    model: Arc<dyn ModelClient>,
    tools: Arc<ToolRegistry>,
    clock: Arc<dyn Clock>,
    middlewares: Vec<Arc<dyn Middleware>>,
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
            middlewares: Vec::new(),
        }
    }

    pub fn with_middleware(mut self, middleware: Arc<dyn Middleware>) -> Self {
        self.middlewares.push(middleware);
        self
    }

    pub fn with_middlewares(
        mut self,
        middlewares: impl IntoIterator<Item = Arc<dyn Middleware>>,
    ) -> Self {
        self.middlewares.extend(middlewares);
        self
    }

    /// Run one turn, recording its events (and so its history) through
    /// `recorder`, whose events are stamped with `turn_id` from here on.
    ///
    /// On error the turn simply stops: the caller records the `Error`, and the
    /// history projection repairs unanswered tool calls when it sees it.
    pub async fn forward(
        &self,
        recorder: &mut Recorder,
        turn_id: TurnId,
        content: String,
        name: Option<String>,
        config: &ThreadConfig,
    ) -> Result<(), CoreError> {
        recorder.set_turn(Some(turn_id));
        recorder
            .record(EventMsg::TurnStarted(TurnStartedEvent {}))
            .await?;
        let turn_ctx = HookCtx {
            thread_id: recorder.thread_id(),
            turn_id,
            iteration: None,
            config,
        };
        for middleware in &self.middlewares {
            middleware.before_turn(&turn_ctx).await?;
        }
        if let Some(context) = next_context_entry(
            recorder.history(),
            config.include_time_context,
            self.clock.as_ref(),
        ) {
            recorder
                .record(EventMsg::EnvironmentContextUpdated(
                    EnvironmentContextUpdatedEvent { context },
                ))
                .await?;
        }
        recorder
            .record(EventMsg::UserMessage(UserMessageEvent { name, content }))
            .await?;

        let mut machine = TurnMachine::new(config.max_tool_iterations);
        let mut total = TokenUsage::default();

        let (finish_reason, last_agent_message) = loop {
            match machine.next_step() {
                TurnStep::CallModel {
                    iteration,
                    allow_tools,
                    reminder,
                } => {
                    recorder
                        .record(EventMsg::ModelRequestStarted(ModelRequestStartedEvent {
                            iteration,
                            allow_tools,
                        }))
                        .await?;

                    if let Some(reminder) = reminder {
                        recorder
                            .record(EventMsg::SystemReminder(SystemReminderEvent {
                                iteration,
                                reminder,
                            }))
                            .await?;
                    }
                    let mut request = ModelRequest {
                        system_prompt: config.effective_system_prompt(),
                        events: recorder.events().await?,
                        tools: if allow_tools {
                            self.tools.specs()
                        } else {
                            Vec::new()
                        },
                        iteration,
                    };
                    let ctx = HookCtx {
                        thread_id: recorder.thread_id(),
                        turn_id,
                        iteration: Some(iteration),
                        config,
                    };
                    for middleware in &self.middlewares {
                        middleware.before_model(&ctx, &mut request).await?;
                    }
                    let (mut response, streamed) = self.stream_model(recorder, request).await?;
                    for middleware in self.middlewares.iter().rev() {
                        middleware.after_model(&ctx, &mut response).await?;
                    }
                    total += response.usage;

                    // Final text wins; deltas only fill in when it is missing.
                    let content = response.content.or(streamed.as_ref().map(|s| s.1.clone()));
                    let started = streamed.as_ref().map(|s| s.0);
                    let (message, effect) = machine.on_model_output(
                        started.unwrap_or_else(MessageId::new),
                        content,
                        response.tool_calls,
                    )?;
                    if let Some(message) = message {
                        if started.is_none() {
                            recorder
                                .record(EventMsg::AssistantMessageStart(
                                    AssistantMessageStartEvent {
                                        message_id: message.message_id,
                                    },
                                ))
                                .await?;
                        }
                        recorder.record(EventMsg::AssistantMessage(message)).await?;
                    }
                    if effect.ignored_tool_calls > 0 {
                        recorder
                            .record(EventMsg::Warning(WarningEvent {
                                message: format!(
                                    "ignored {} tool call(s) requested after the iteration limit",
                                    effect.ignored_tool_calls
                                ),
                            }))
                            .await?;
                    }

                    if let TurnStep::RunTools { calls } = machine.next_step() {
                        let mut results = Vec::with_capacity(calls.len());
                        for call in calls {
                            results.push(
                                self.run_tool(recorder, call, config, turn_id, iteration)
                                    .await?,
                            );
                        }
                        machine.on_tool_results(&results)?;
                    }

                    recorder
                        .record(EventMsg::ModelRequestCompleted(
                            ModelRequestCompletedEvent {
                                iteration,
                                usage: response.usage,
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

        for middleware in self.middlewares.iter().rev() {
            middleware.after_turn(&turn_ctx).await?;
        }
        recorder
            .record(EventMsg::TokenCount(TokenCountEvent { usage: total }))
            .await?;
        recorder
            .record(EventMsg::TurnComplete(TurnCompleteEvent {
                last_agent_message,
                finish_reason,
            }))
            .await?;
        Ok(())
    }

    /// Reads one streamed model call, recording `AssistantMessageStart` at the
    /// first non-empty delta and an `AssistantMessageDelta` per delta.
    ///
    /// Returns the final response and, if any delta arrived, the message id and
    /// the concatenated delta text.
    async fn stream_model(
        &self,
        recorder: &mut Recorder,
        request: ModelRequest,
    ) -> Result<(ModelResponse, Option<(MessageId, String)>), CoreError> {
        let mut stream = self.model.stream(request).await?;
        let mut streamed: Option<(MessageId, String)> = None;
        while let Some(item) = stream.next().await {
            match item? {
                ModelStreamItem::TextDelta { text } if text.is_empty() => {}
                ModelStreamItem::TextDelta { text } => {
                    let message_id = match &mut streamed {
                        Some((id, buffer)) => {
                            buffer.push_str(&text);
                            *id
                        }
                        None => {
                            let id = MessageId::new();
                            recorder
                                .record(EventMsg::AssistantMessageStart(
                                    AssistantMessageStartEvent { message_id: id },
                                ))
                                .await?;
                            streamed = Some((id, text.clone()));
                            id
                        }
                    };
                    recorder
                        .record(EventMsg::AssistantMessageDelta(
                            AssistantMessageDeltaEvent {
                                message_id,
                                delta: text,
                            },
                        ))
                        .await?;
                }
                ModelStreamItem::Completed { response } => return Ok((response, streamed)),
            }
        }
        Err(CoreError::Model(
            "model stream ended without a final response".into(),
        ))
    }

    async fn run_tool(
        &self,
        recorder: &mut Recorder,
        mut call: ToolCall,
        config: &ThreadConfig,
        turn_id: TurnId,
        iteration: u32,
    ) -> Result<ToolResult, CoreError> {
        let original_id = call.id.clone();
        let original_name = call.name.clone();
        let ctx = HookCtx {
            thread_id: recorder.thread_id(),
            turn_id,
            iteration: Some(iteration),
            config,
        };
        let mut decision = ToolDecision::Continue;
        for middleware in &self.middlewares {
            decision = middleware.before_tool(&ctx, &mut call).await?;
            if matches!(&decision, ToolDecision::Skip(_)) {
                break;
            }
        }
        call.id = original_id;
        call.name = original_name;
        recorder
            .record(EventMsg::ToolCallBegin(ToolCallBeginEvent {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                arguments: call.arguments.clone(),
            }))
            .await?;

        let started = Instant::now();
        let skipped = matches!(&decision, ToolDecision::Skip(_));
        let mut result = match decision {
            ToolDecision::Skip(result) => result,
            ToolDecision::Continue => match self.tools.get(&call.name) {
                Some(tool) => tool.execute(&call).await,
                None => ToolResult::not_found(&call),
            },
        };
        for middleware in self.middlewares.iter().rev() {
            middleware.after_tool(&ctx, &call, &mut result).await?;
        }
        // The call is authoritative; middleware and tools cannot re-key results.
        result.call_id = call.id.clone();
        result.tool_name = call.name.clone();
        let duration_ms = if skipped {
            0
        } else {
            started.elapsed().as_millis() as u64
        };

        recorder
            .record(EventMsg::ToolCallEnd(ToolCallEndEvent {
                result: result.clone(),
                duration_ms: Some(duration_ms),
            }))
            .await?;

        Ok(result)
    }
}

#[async_trait::async_trait]
impl Agent for AgentLoop {
    async fn forward(
        &self,
        recorder: &mut Recorder,
        turn_id: TurnId,
        content: String,
        name: Option<String>,
        config: &ThreadConfig,
    ) -> Result<(), CoreError> {
        AgentLoop::forward(self, recorder, turn_id, content, name, config).await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;

    use async_trait::async_trait;
    use serde_json::Value;
    use tapestry_protocol::{
        EnvironmentContext, Event, FinishReason, HistoryItem, ModelResponse, SubmissionId,
        SystemReminder, ThreadId, ToolCallId, ToolCallStatus, ToolSpec,
    };

    use super::*;
    use crate::clock::FixedClock;
    use crate::formatter::formatter_trait::HistoryProjection;
    use crate::model::{MockModelClient, ModelStream};
    use crate::store::testing::FlakyStore;
    use crate::store::{EventStore, InMemoryEventStore};
    use crate::tool::{EchoTool, Tool};

    fn request_history(request: &ModelRequest) -> Vec<HistoryItem> {
        HistoryProjection::fold(&request.events).history().to_vec()
    }

    fn tc(name: &str, arguments: Value) -> ToolCall {
        ToolCall::new(ToolCallId::generate(), name, arguments)
    }

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

    /// Exposes only `complete`, so the default (delta-free) `stream` is used.
    struct NonStreaming<M>(M);

    #[async_trait]
    impl<M: ModelClient> ModelClient for NonStreaming<M> {
        async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, CoreError> {
            self.0.complete(request).await
        }
    }

    /// Replays one scripted stream per model call.
    struct StreamScript(Mutex<VecDeque<Vec<Result<ModelStreamItem, CoreError>>>>);

    impl StreamScript {
        fn new(calls: Vec<Vec<Result<ModelStreamItem, CoreError>>>) -> Arc<Self> {
            Arc::new(Self(Mutex::new(calls.into())))
        }
    }

    #[async_trait]
    impl ModelClient for StreamScript {
        async fn complete(&self, _request: ModelRequest) -> Result<ModelResponse, CoreError> {
            unreachable!("the agent loop only streams")
        }

        async fn stream(&self, _request: ModelRequest) -> Result<ModelStream, CoreError> {
            let items = self
                .0
                .lock()
                .unwrap()
                .pop_front()
                .expect("script exhausted");
            Ok(ModelStream::from_items(items))
        }
    }

    fn delta(text: &str) -> Result<ModelStreamItem, CoreError> {
        Ok(ModelStreamItem::TextDelta { text: text.into() })
    }

    fn done(response: Result<ModelResponse, CoreError>) -> Result<ModelStreamItem, CoreError> {
        Ok(ModelStreamItem::Completed {
            response: response.unwrap(),
        })
    }

    /// `(message_id, kind)` of every assistant message event.
    fn message_events(events: &[Event]) -> Vec<(MessageId, String)> {
        events
            .iter()
            .filter_map(|e| match &e.msg {
                EventMsg::AssistantMessageStart(m) => Some((m.message_id, "start".into())),
                EventMsg::AssistantMessageDelta(m) => {
                    Some((m.message_id, format!("delta:{}", m.delta)))
                }
                EventMsg::AssistantMessage(m) => Some((
                    m.message_id,
                    format!("message:{}", m.content.clone().unwrap_or_default()),
                )),
                _ => None,
            })
            .collect()
    }

    fn resp(content: Option<&str>, calls: Vec<ToolCall>) -> Result<ModelResponse, CoreError> {
        Ok(ModelResponse {
            content: content.map(str::to_string),
            tool_calls: calls,
            usage: TokenUsage {
                input_tokens: 1,
                output_tokens: 2,
                ..Default::default()
            },
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
        async fn execute(&self, call: &ToolCall) -> ToolResult {
            ToolResult::failed(call, "nope")
        }
    }

    /// Returns a result keyed by an id and name that do not belong to the call.
    struct MisKeyedTool;

    #[async_trait]
    impl Tool for MisKeyedTool {
        fn name(&self) -> &str {
            "miskeyed"
        }
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "miskeyed".into(),
                description: "returns a foreign id".into(),
                parameters: serde_json::json!({"type": "object"}),
            }
        }
        async fn execute(&self, _call: &ToolCall) -> ToolResult {
            ToolResult {
                call_id: ToolCallId::from("foreign"),
                tool_name: "other".into(),
                output: "ok".into(),
                status: ToolCallStatus::Completed,
            }
        }
    }

    struct CountingEchoTool(Arc<AtomicUsize>);

    #[async_trait]
    impl Tool for CountingEchoTool {
        fn name(&self) -> &str {
            "echo"
        }

        fn spec(&self) -> ToolSpec {
            EchoTool.spec()
        }

        async fn execute(&self, call: &ToolCall) -> ToolResult {
            self.0.fetch_add(1, Ordering::SeqCst);
            EchoTool.execute(call).await
        }
    }

    fn registry() -> Arc<ToolRegistry> {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        registry.register(Arc::new(FailingTool));
        registry.register(Arc::new(MisKeyedTool));
        Arc::new(registry)
    }

    struct Probe {
        label: &'static str,
        trace: Arc<Mutex<Vec<String>>>,
        prompt_suffix: Option<&'static str>,
        uppercase_response: bool,
        tool_input: Option<&'static str>,
        append_tool_input: Option<&'static str>,
        skip_output: Option<&'static str>,
        rekey_call: bool,
        after_output: Option<&'static str>,
        rekey_result: bool,
        fail_before_model: bool,
        tool_count: Option<Arc<AtomicUsize>>,
        event_store: Option<Arc<dyn EventStore>>,
        turn_complete_seen: Option<Arc<AtomicBool>>,
    }

    impl Probe {
        fn new(label: &'static str, trace: Arc<Mutex<Vec<String>>>) -> Self {
            Self {
                label,
                trace,
                prompt_suffix: None,
                uppercase_response: false,
                tool_input: None,
                append_tool_input: None,
                skip_output: None,
                rekey_call: false,
                after_output: None,
                rekey_result: false,
                fail_before_model: false,
                tool_count: None,
                event_store: None,
                turn_complete_seen: None,
            }
        }

        fn record(&self, hook: &str) {
            self.trace
                .lock()
                .unwrap()
                .push(format!("{}.{}", self.label, hook));
        }
    }

    #[async_trait]
    impl Middleware for Probe {
        async fn before_turn(&self, _ctx: &HookCtx<'_>) -> Result<(), CoreError> {
            self.record("before_turn");
            Ok(())
        }

        async fn after_turn(&self, ctx: &HookCtx<'_>) -> Result<(), CoreError> {
            self.record("after_turn");
            if let (Some(store), Some(seen)) = (&self.event_store, &self.turn_complete_seen) {
                let events = store.read_after(ctx.thread_id, 0).await?;
                seen.store(
                    events.iter().any(|event| {
                        matches!(
                            event.msg,
                            EventMsg::TokenCount(_) | EventMsg::TurnComplete(_)
                        )
                    }),
                    Ordering::SeqCst,
                );
            }
            Ok(())
        }

        async fn before_model(
            &self,
            _ctx: &HookCtx<'_>,
            request: &mut ModelRequest,
        ) -> Result<(), CoreError> {
            self.record("before_model");
            if self.fail_before_model {
                return Err(CoreError::Tool("middleware before_model failed".into()));
            }
            if let Some(suffix) = self.prompt_suffix {
                request
                    .system_prompt
                    .get_or_insert_with(String::new)
                    .push_str(suffix);
            }
            Ok(())
        }

        async fn after_model(
            &self,
            _ctx: &HookCtx<'_>,
            response: &mut ModelResponse,
        ) -> Result<(), CoreError> {
            self.record("after_model");
            if self.uppercase_response {
                if let Some(content) = &mut response.content {
                    content.make_ascii_uppercase();
                }
            }
            Ok(())
        }

        async fn before_tool(
            &self,
            _ctx: &HookCtx<'_>,
            call: &mut ToolCall,
        ) -> Result<ToolDecision, CoreError> {
            self.record("before_tool");
            if let Some(count) = &self.tool_count {
                count.fetch_add(1, Ordering::SeqCst);
            }
            if let Some(input) = self.tool_input {
                call.arguments["input"] = serde_json::json!(input);
            }
            if let Some(suffix) = self.append_tool_input {
                let input = call.arguments["input"].as_str().unwrap_or_default();
                call.arguments["input"] = serde_json::json!(format!("{input}{suffix}"));
            }
            if self.rekey_call {
                call.id = ToolCallId::from("middleware-id");
                call.name = "middleware-tool".into();
            }
            if let Some(output) = self.skip_output {
                return Ok(ToolDecision::Skip(ToolResult::completed(call, output)));
            }
            Ok(ToolDecision::Continue)
        }

        async fn after_tool(
            &self,
            _ctx: &HookCtx<'_>,
            _call: &ToolCall,
            result: &mut ToolResult,
        ) -> Result<(), CoreError> {
            self.record("after_tool");
            if let Some(output) = self.after_output {
                result.output = output.into();
            }
            if self.rekey_result {
                result.call_id = ToolCallId::from("middleware-result-id");
                result.tool_name = "middleware-result-tool".into();
            }
            Ok(())
        }
    }

    struct Harness {
        agent: AgentLoop,
        store: Arc<dyn EventStore>,
        recorder: Recorder,
        config: ThreadConfig,
        thread: ThreadId,
    }

    impl Harness {
        fn with_store(
            model: Arc<dyn ModelClient>,
            clock: Arc<FixedClock>,
            config: ThreadConfig,
            store: Arc<dyn EventStore>,
        ) -> Self {
            let thread = ThreadId::new();
            Self {
                agent: AgentLoop::with_clock(model, registry(), clock.clone()),
                recorder: Recorder::new(store.clone(), clock, thread),
                store,
                config,
                thread,
            }
        }

        fn new(model: Arc<dyn ModelClient>, clock: Arc<FixedClock>, config: ThreadConfig) -> Self {
            Self::with_store(model, clock, config, Arc::new(InMemoryEventStore::new()))
        }

        /// Time context has dedicated tests; elsewhere keep the history exact.
        fn without_time(model: Arc<dyn ModelClient>, config: &ThreadConfig) -> Self {
            Self::new(
                model,
                Arc::new(FixedClock::new("2026-11-04", "UTC")),
                ThreadConfig {
                    include_time_context: false,
                    ..config.clone()
                },
            )
        }

        async fn log(&self) -> Vec<Event> {
            self.store.read_after(self.thread, 0).await.unwrap()
        }

        async fn turn(
            &mut self,
            message: &str,
        ) -> (Result<(), CoreError>, Vec<Event>, SubmissionId) {
            let before = self.log().await.len() as u64;
            let sid = SubmissionId::new();
            self.recorder.begin_submission(sid);
            let result = self
                .agent
                .forward(
                    &mut self.recorder,
                    TurnId::new(),
                    message.into(),
                    None,
                    &self.config,
                )
                .await;
            let events = self.store.read_after(self.thread, before).await.unwrap();
            (result, events, sid)
        }

        fn history(&self) -> &[HistoryItem] {
            self.recorder.history()
        }

        async fn folded(&self) -> Vec<HistoryItem> {
            HistoryProjection::fold(&self.log().await)
                .history()
                .to_vec()
        }

        fn context_entries(&self) -> Vec<&EnvironmentContext> {
            self.history()
                .iter()
                .filter_map(|i| match i {
                    HistoryItem::EnvironmentContext(context) => Some(context),
                    _ => None,
                })
                .collect()
        }
    }

    async fn run(
        model: Arc<dyn ModelClient>,
        message: &str,
        config: &ThreadConfig,
    ) -> (Result<(), CoreError>, Vec<Event>, Harness) {
        let mut h = Harness::without_time(model, config);
        let (result, events, _) = h.turn(message).await;
        (result, events, h)
    }

    fn kinds(events: &[Event]) -> Vec<String> {
        events
            .iter()
            .map(|e| match &e.msg {
                EventMsg::TurnStarted(_) => "turn_started".into(),
                EventMsg::EnvironmentContextUpdated(_) => "context".into(),
                EventMsg::UserMessage(_) => "user".into(),
                EventMsg::SystemReminder(_) => "reminder".into(),
                EventMsg::ModelRequestStarted(m) => format!("req_start:{}", m.iteration),
                EventMsg::ModelRequestCompleted(m) => format!("req_done:{}", m.iteration),
                EventMsg::AssistantMessageStart(_) => "start".into(),
                EventMsg::AssistantMessageDelta(_) => "delta".into(),
                EventMsg::AssistantMessage(m) => {
                    format!("assistant:{}", m.tool_calls.len())
                }
                EventMsg::ToolCallBegin(_) => "tool_begin".into(),
                EventMsg::ToolCallEnd(t) => format!("tool_end:{:?}", t.result.status),
                EventMsg::Warning(_) => "warning".into(),
                EventMsg::TokenCount(_) => "token_count".into(),
                EventMsg::TurnComplete(_) => "turn_complete".into(),
                other => format!("{other:?}"),
            })
            .collect()
    }

    fn assert_history_valid(history: &[HistoryItem]) {
        for item in history {
            if let HistoryItem::Assistant { tool_calls, .. } = item {
                for call in tool_calls {
                    assert!(
                        history.iter().any(
                            |i| matches!(i, HistoryItem::ToolResult(r) if r.call_id == call.id)
                        ),
                        "no result for {:?}",
                        call.id
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn direct_answer_event_order() {
        let (result, events, h) =
            run(Arc::new(MockModelClient), "hello", &ThreadConfig::default()).await;
        result.unwrap();
        assert_eq!(
            kinds(&events),
            [
                "turn_started",
                "user",
                "req_start:0",
                "start",
                "delta",
                "delta",
                "assistant:0",
                "req_done:0",
                "token_count",
                "turn_complete"
            ]
        );
        match &events.last().unwrap().msg {
            EventMsg::TurnComplete(c) => {
                assert_eq!(c.finish_reason, FinishReason::Completed);
                assert_eq!(c.last_agent_message.as_deref(), Some("Echo: hello"));
            }
            _ => panic!(),
        }
        assert_eq!(h.history().len(), 2);
    }

    #[tokio::test]
    async fn tool_turn_event_order_envelope_and_history() {
        let mut h = Harness::without_time(
            Arc::new(NonStreaming(MockModelClient)),
            &ThreadConfig::default(),
        );
        let (result, events, sid) = h.turn("please tool:echo now").await;
        result.unwrap();
        assert_eq!(
            kinds(&events),
            [
                "turn_started",
                "user",
                "req_start:0",
                "start",
                "assistant:1",
                "tool_begin",
                "tool_end:Completed",
                "req_done:0",
                "req_start:1",
                "start",
                "assistant:0",
                "req_done:1",
                "token_count",
                "turn_complete"
            ]
        );
        let positions: Vec<_> = events.iter().map(|e| e.log_position).collect();
        assert_eq!(positions, (1..=14).collect::<Vec<_>>());
        let turn_id = events[0].turn_id;
        assert!(turn_id.is_some());
        assert!(events
            .iter()
            .all(|e| e.id == sid && e.turn_id == turn_id && e.thread_id == h.thread));

        let (begin, end) = events.iter().fold((None, None), |acc, e| match &e.msg {
            EventMsg::ToolCallBegin(b) => (Some(b.tool_call_id.clone()), acc.1),
            EventMsg::ToolCallEnd(t) => (acc.0, Some((t.result.call_id.clone(), t.duration_ms))),
            _ => acc,
        });
        assert_eq!(begin, end.clone().map(|e| e.0));
        assert!(end.unwrap().1.is_some());

        let history = h.history();
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
        assert_eq!(h.folded().await, history);
    }

    #[tokio::test]
    async fn streaming_mock_tool_turn_emits_deltas_with_one_message_id() {
        let mut h = Harness::without_time(Arc::new(MockModelClient), &ThreadConfig::default());
        let (result, events, _) = h.turn("please tool:echo now").await;
        result.unwrap();
        let kinds = kinds(&events);
        // The tool-call output has no text, so no deltas before it.
        assert_eq!(kinds[3..5], ["start", "assistant:1"]);
        assert!(kinds.iter().filter(|k| *k == "delta").count() > 1);

        let messages = message_events(&events);
        let second = &messages[2..];
        assert_eq!(second[0].1, "start");
        assert!(second.iter().all(|(id, _)| *id == second[0].0));
        assert_ne!(messages[0].0, second[0].0);
        let joined: String = second
            .iter()
            .filter_map(|(_, k)| k.strip_prefix("delta:"))
            .collect();
        assert_eq!(second.last().unwrap().1, format!("message:{joined}"));
        assert_eq!(h.folded().await, h.history());
    }

    #[tokio::test]
    async fn streamed_text_shares_message_id() {
        let model = StreamScript::new(vec![vec![
            delta("Hel"),
            delta("lo"),
            done(resp(Some("Hello"), vec![])),
        ]]);
        let (result, events, _) = run(model, "hi", &ThreadConfig::default()).await;
        result.unwrap();
        let messages = message_events(&events);
        let kinds: Vec<_> = messages.iter().map(|m| m.1.as_str()).collect();
        assert_eq!(kinds, ["start", "delta:Hel", "delta:lo", "message:Hello"]);
        assert!(messages.iter().all(|m| m.0 == messages[0].0));
    }

    #[tokio::test]
    async fn non_streaming_client_emits_start_then_message() {
        let model = ScriptedModel::new(vec![resp(Some("hi"), vec![])]);
        let (result, events, _) = run(model, "go", &ThreadConfig::default()).await;
        result.unwrap();
        let messages = message_events(&events);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].1, "start");
        assert_eq!(messages[1].1, "message:hi");
        assert_eq!(messages[0].0, messages[1].0);
    }

    #[tokio::test]
    async fn final_text_wins_over_deltas() {
        let model = StreamScript::new(vec![vec![delta("Hi"), done(resp(Some("Hi!"), vec![]))]]);
        let (result, events, h) = run(model, "go", &ThreadConfig::default()).await;
        result.unwrap();
        assert_eq!(message_events(&events).last().unwrap().1, "message:Hi!");
        assert_eq!(
            h.history().last(),
            Some(&HistoryItem::assistant(Some("Hi!".into()), vec![]))
        );
    }

    #[tokio::test]
    async fn missing_final_text_falls_back_to_deltas() {
        let model = StreamScript::new(vec![vec![delta("H"), delta("i"), done(resp(None, vec![]))]]);
        let (result, events, _) = run(model, "go", &ThreadConfig::default()).await;
        result.unwrap();
        assert_eq!(message_events(&events).last().unwrap().1, "message:Hi");
        match &events.last().unwrap().msg {
            EventMsg::TurnComplete(c) => assert_eq!(c.last_agent_message.as_deref(), Some("Hi")),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn empty_output_emits_no_message_events() {
        let model = StreamScript::new(vec![vec![delta(""), done(resp(None, vec![]))]]);
        let (result, events, _) = run(model, "go", &ThreadConfig::default()).await;
        result.unwrap();
        assert!(message_events(&events).is_empty());
    }

    #[tokio::test]
    async fn stream_error_after_delta_leaves_no_assistant_message() {
        let model = StreamScript::new(vec![vec![
            delta("Hel"),
            Err(CoreError::Model("cut".into())),
        ]]);
        let (result, events, h) = run(model, "go", &ThreadConfig::default()).await;
        assert!(matches!(result, Err(CoreError::Model(_))));
        let kinds: Vec<_> = message_events(&events).into_iter().map(|m| m.1).collect();
        assert_eq!(kinds, ["start", "delta:Hel"]);
        assert_eq!(h.history(), [HistoryItem::user(None, "go")]);
    }

    #[tokio::test]
    async fn stream_ending_without_final_response_is_a_model_error() {
        let model = StreamScript::new(vec![vec![delta("Hel")]]);
        let (result, _, _) = run(model, "go", &ThreadConfig::default()).await;
        assert!(matches!(result, Err(CoreError::Model(_))));
    }

    #[tokio::test]
    async fn user_message_event_carries_name() {
        let mut h = Harness::without_time(Arc::new(MockModelClient), &ThreadConfig::default());
        h.recorder.begin_submission(SubmissionId::new());
        h.agent
            .forward(
                &mut h.recorder,
                TurnId::new(),
                "hi".into(),
                Some("alice".into()),
                &h.config,
            )
            .await
            .unwrap();
        let user = h
            .log()
            .await
            .into_iter()
            .find_map(|e| match e.msg {
                EventMsg::UserMessage(m) => Some(m),
                _ => None,
            })
            .unwrap();
        assert_eq!(user.name.as_deref(), Some("alice"));
        assert_eq!(user.content, "hi");
    }

    #[tokio::test]
    async fn unknown_and_failing_tools_are_fed_back() {
        let ghost = tc("ghost", Value::Null);
        let fail = tc("fail", Value::Null);
        let model = ScriptedModel::new(vec![
            resp(Some("thinking"), vec![ghost.clone(), fail.clone()]),
            resp(Some("recovered"), vec![]),
        ]);
        let (result, events, _) = run(model.clone(), "go", &ThreadConfig::default()).await;
        result.unwrap();
        assert_eq!(
            kinds(&events),
            [
                "turn_started",
                "user",
                "req_start:0",
                "start",
                "assistant:2",
                "tool_begin",
                "tool_end:NotFound",
                "tool_begin",
                "tool_end:Failed",
                "req_done:0",
                "req_start:1",
                "start",
                "assistant:0",
                "req_done:1",
                "token_count",
                "turn_complete"
            ]
        );
        let ends: Vec<_> = events
            .iter()
            .filter_map(|e| match &e.msg {
                EventMsg::ToolCallEnd(t) => Some((
                    t.result.call_id.clone(),
                    t.result.status,
                    t.result.output.clone(),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(ends[0].0, ghost.id);
        assert_eq!(ends[0].1, ToolCallStatus::NotFound);
        assert!(ends[0].2.contains("ghost"));
        assert_eq!(ends[1].0, fail.id);
        assert_eq!(ends[1].2, "nope");

        let requests = model.requests.lock().unwrap();
        assert_eq!(request_history(&requests[1]).len(), 4);
        assert_eq!(requests[0].tools.len(), 3);
    }

    #[tokio::test]
    async fn exceeding_iterations_forces_tool_free_finish() {
        let echo = || tc("echo", serde_json::json!({"input": "x"}));
        let model = ScriptedModel::new(vec![
            resp(None, vec![echo()]),
            resp(None, vec![echo()]),
            resp(Some("summary"), vec![]),
        ]);
        let config = ThreadConfig {
            max_tool_iterations: 2,
            ..Default::default()
        };
        let (result, events, h) = run(model.clone(), "go", &config).await;
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

        // The reminder follows the forced call's ModelRequestStarted.
        let k = kinds(&events);
        let at = k.iter().position(|s| s == "req_start:2").unwrap();
        assert_eq!(k[at + 1], "reminder");
        assert_eq!(k.iter().filter(|s| *s == "reminder").count(), 1);
        match &events[at + 1].msg {
            EventMsg::SystemReminder(r) => {
                assert_eq!(r.iteration, 2);
                assert_eq!(r.reminder, SystemReminder::ToolLimitReached);
            }
            _ => panic!(),
        }
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
        assert_eq!(
            requests[2]
                .events
                .iter()
                .filter(|event| matches!(event.msg, EventMsg::SystemReminder(_)))
                .count(),
            1
        );
        let forced_history = request_history(&requests[2]);
        assert_eq!(
            forced_history.last(),
            Some(&HistoryItem::system_reminder(
                SystemReminder::ToolLimitReached
            ))
        );
        assert_eq!(forced_history.len(), 6);
        // The reminder stays in the event projection and is filtered for later requests.
        let history = h.history();
        assert_eq!(history.len(), 7);
        assert!(history
            .iter()
            .any(|i| matches!(i, HistoryItem::SystemReminder(_))));
        assert!(matches!(
            &history[6],
            HistoryItem::Assistant { content: Some(c), .. } if c == "summary"
        ));
    }

    #[tokio::test]
    async fn forced_finish_tool_calls_warn_and_are_ignored() {
        let model = ScriptedModel::new(vec![
            resp(None, vec![tc("echo", Value::Null)]),
            resp(Some("still"), vec![tc("echo", Value::Null)]),
        ]);
        let config = ThreadConfig {
            max_tool_iterations: 1,
            ..Default::default()
        };
        let (result, events, h) = run(model, "go", &config).await;
        result.unwrap();
        let k = kinds(&events);
        assert_eq!(k.iter().filter(|s| *s == "warning").count(), 1);
        assert_eq!(k.iter().filter(|s| s.starts_with("tool_begin")).count(), 1);
        assert_eq!(k.last().unwrap(), "turn_complete");
        let last_assistant = events
            .iter()
            .rev()
            .find_map(|e| match &e.msg {
                EventMsg::AssistantMessage(m) => Some(m),
                _ => None,
            })
            .unwrap();
        assert_eq!(last_assistant.content.as_deref(), Some("still"));
        assert!(last_assistant.tool_calls.is_empty());
        assert!(matches!(
            h.history().last(),
            Some(HistoryItem::Assistant { tool_calls, .. }) if tool_calls.is_empty()
        ));
    }

    #[tokio::test]
    async fn model_failure_returns_error_and_folded_history_is_valid() {
        let model = ScriptedModel::new(vec![
            resp(None, vec![tc("echo", Value::Null)]),
            Err(CoreError::Model("down".into())),
        ]);
        let (result, events, h) = run(model, "go", &ThreadConfig::default()).await;
        assert!(matches!(result, Err(CoreError::Model(_))));
        assert!(!kinds(&events).contains(&"turn_complete".to_string()));
        assert_history_valid(&h.folded().await);
    }

    #[tokio::test]
    async fn model_failure_on_tool_less_turn_leaves_user_message() {
        let model = ScriptedModel::new(vec![Err(CoreError::Model("down".into()))]);
        let (result, _, h) = run(model, "go", &ThreadConfig::default()).await;
        assert!(result.is_err());
        assert_eq!(h.history().len(), 1);
    }

    #[tokio::test]
    async fn store_failure_terminates_turn_without_further_appends() {
        let store = Arc::new(FlakyStore::default());
        store.fail_after(1);
        let mut h = Harness::with_store(
            Arc::new(MockModelClient),
            Arc::new(FixedClock::new("2026-11-04", "UTC")),
            ThreadConfig {
                include_time_context: false,
                ..Default::default()
            },
            store.clone(),
        );
        let (result, events, _) = h.turn("tool:echo").await;
        assert!(matches!(result, Err(CoreError::EventStore(_))));
        assert_eq!(kinds(&events), ["turn_started"]);
        // Nothing that failed to append was projected.
        assert!(h.history().is_empty());
    }

    #[tokio::test]
    async fn store_failure_mid_tool_run_leaves_valid_folded_history() {
        // turn_started, user, req_start, start, assistant(2 calls), tool_begin, then fail.
        let store = Arc::new(FlakyStore::default());
        store.fail_after(6);
        let mut h = Harness::with_store(
            ScriptedModel::new(vec![resp(
                None,
                vec![tc("echo", Value::Null), tc("echo", Value::Null)],
            )]),
            Arc::new(FixedClock::new("2026-11-04", "UTC")),
            ThreadConfig {
                include_time_context: false,
                ..Default::default()
            },
            store,
        );
        let (result, events, _) = h.turn("go").await;
        assert!(matches!(result, Err(CoreError::EventStore(_))));
        assert_eq!(kinds(&events).last().unwrap(), "tool_begin");
        let folded = h.folded().await;
        assert_history_valid(&folded);
        let results = folded
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
        let (result, _, _) = run(model.clone(), "go", &config).await;
        result.unwrap();
        let prompt = model.requests.lock().unwrap()[0]
            .system_prompt
            .clone()
            .unwrap();
        assert!(prompt.starts_with("Base.") && prompt.contains("planner"));
    }

    struct TimeHarness {
        h: Harness,
        model: Arc<ScriptedModel>,
        clock: Arc<FixedClock>,
    }

    impl TimeHarness {
        fn new(turns: usize) -> Self {
            let model = ScriptedModel::new((0..turns).map(|_| resp(Some("ok"), vec![])).collect());
            let clock = Arc::new(FixedClock::new("2026-11-04", "Asia/Shanghai"));
            Self {
                h: Harness::new(model.clone(), clock.clone(), ThreadConfig::default()),
                model,
                clock,
            }
        }

        async fn turn(&mut self, message: &str) -> (Result<(), CoreError>, Vec<Event>) {
            let (result, events, _) = self.h.turn(message).await;
            (result, events)
        }
    }

    fn available(date: &str, timezone: &str) -> EnvironmentContext {
        EnvironmentContext::Available {
            date: date.into(),
            timezone: timezone.into(),
        }
    }

    #[tokio::test]
    async fn first_turn_puts_time_context_before_user_message() {
        let mut t = TimeHarness::new(1);
        let (result, events) = t.turn("hello").await;
        result.unwrap();
        match (&t.h.history()[0], &t.h.history()[1]) {
            (HistoryItem::EnvironmentContext(ctx), HistoryItem::User { content: msg, .. }) => {
                assert_eq!(ctx, &available("2026-11-04", "Asia/Shanghai"));
                assert_eq!(msg, "hello");
            }
            other => panic!("unexpected history: {other:?}"),
        }
        let request = t.model.requests.lock().unwrap()[0].clone();
        let sent = request_history(&request);
        assert!(matches!(&sent[0], HistoryItem::EnvironmentContext(_)));

        assert_eq!(
            kinds(&events),
            vec![
                "turn_started",
                "context",
                "user",
                "req_start:0",
                "start",
                "assistant:0",
                "req_done:0",
                "token_count",
                "turn_complete"
            ]
        );
        match &events[1].msg {
            EventMsg::EnvironmentContextUpdated(e) => {
                assert_eq!(e.context, available("2026-11-04", "Asia/Shanghai"))
            }
            _ => panic!(),
        }
    }

    #[tokio::test]
    async fn same_day_turn_does_not_resend_and_next_day_sends_full_snapshot() {
        let mut t = TimeHarness::new(3);
        t.turn("a").await.0.unwrap();
        let (result, events) = t.turn("b").await;
        result.unwrap();
        assert!(!kinds(&events).contains(&"context".to_string()));
        assert_eq!(t.h.context_entries().len(), 1);
        t.clock.set("2026-11-05", "Asia/Shanghai");
        t.turn("c").await.0.unwrap();
        let entries = t.h.context_entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1], &available("2026-11-05", "Asia/Shanghai"));
    }

    #[tokio::test]
    async fn disabled_from_start_never_reads_clock_or_injects() {
        let mut t = TimeHarness::new(1);
        t.h.config.include_time_context = false;
        t.clock.fail();
        let (result, _) = t.turn("hi").await;
        result.unwrap();
        assert_eq!(t.h.history().len(), 2);
        assert!(t.h.context_entries().is_empty());
    }

    #[tokio::test]
    async fn disabling_midway_sends_unavailable_once() {
        let mut t = TimeHarness::new(3);
        t.turn("a").await.0.unwrap();
        t.h.config.include_time_context = false;
        t.turn("b").await.0.unwrap();
        t.turn("c").await.0.unwrap();
        let entries = t.h.context_entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1], &EnvironmentContext::Unavailable);
    }

    #[tokio::test]
    async fn clock_failure_does_not_fail_turn_and_recovers_fully() {
        let mut t = TimeHarness::new(3);
        t.clock.fail();
        let (result, events) = t.turn("a").await;
        result.unwrap();
        assert!(!events.iter().any(|e| matches!(e.msg, EventMsg::Error(_))));
        t.turn("b").await.0.unwrap();
        assert_eq!(t.h.context_entries().len(), 1);
        t.clock.set("2026-11-04", "UTC");
        t.turn("c").await.0.unwrap();
        let entries = t.h.context_entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1], &available("2026-11-04", "UTC"));
    }

    #[tokio::test]
    async fn failed_turn_keeps_context_entry_and_valid_history() {
        let model = ScriptedModel::new(vec![
            Err(CoreError::Model("down".into())),
            resp(Some("ok"), vec![]),
        ]);
        let mut h = Harness::new(
            model,
            Arc::new(FixedClock::new("2026-11-04", "UTC")),
            ThreadConfig::default(),
        );
        assert!(h.turn("a").await.0.is_err());
        assert_eq!(h.history().len(), 2);
        h.turn("b").await.0.unwrap();
        assert_eq!(h.context_entries().len(), 1);
    }

    #[tokio::test]
    async fn events_carry_clock_timestamp() {
        let clock = Arc::new(FixedClock::new("2026-11-04", "UTC"));
        clock.set_unix_millis(1_234);
        let mut h = Harness::new(Arc::new(MockModelClient), clock, ThreadConfig::default());
        let (_, events, _) = h.turn("hi").await;
        assert!(events.iter().all(|e| e.timestamp_ms == 1_234));
    }

    #[tokio::test]
    async fn tool_result_is_keyed_by_the_original_call() {
        let call = tc("miskeyed", Value::Null);
        let model = ScriptedModel::new(vec![
            resp(None, vec![call.clone()]),
            resp(Some("done"), vec![]),
        ]);
        let (result, events, h) = run(model, "go", &ThreadConfig::default()).await;
        result.unwrap();

        let end = events
            .iter()
            .find_map(|e| match &e.msg {
                EventMsg::ToolCallEnd(t) => Some(t),
                _ => None,
            })
            .unwrap();
        assert_eq!(end.result.call_id, call.id);
        assert_eq!(end.result.tool_name, "miskeyed");
        assert!(h.history().iter().any(|i| matches!(
            i,
            HistoryItem::ToolResult(r) if r.call_id == call.id && r.tool_name == "miskeyed"
        )));
        assert_eq!(kinds(&events).last().unwrap(), "turn_complete");
    }

    #[tokio::test]
    async fn turn_hooks_bracket_other_hooks_and_precede_turn_complete() {
        let trace = Arc::new(Mutex::new(Vec::new()));
        let turn_complete_seen = Arc::new(AtomicBool::new(false));
        let mut h = Harness::without_time(Arc::new(MockModelClient), &ThreadConfig::default());
        let mut probe = Probe::new("one", trace.clone());
        probe.event_store = Some(h.store.clone());
        probe.turn_complete_seen = Some(turn_complete_seen.clone());
        h.agent = h.agent.with_middleware(Arc::new(probe));

        h.turn("please tool:echo now").await.0.unwrap();

        assert_eq!(
            *trace.lock().unwrap(),
            [
                "one.before_turn",
                "one.before_model",
                "one.after_model",
                "one.before_tool",
                "one.after_tool",
                "one.before_model",
                "one.after_model",
                "one.after_turn"
            ]
        );
        assert!(!turn_complete_seen.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn before_model_rewrites_request_without_emitting_events() {
        let model = ScriptedModel::new(vec![resp(Some("ok"), vec![])]);
        let mut h = Harness::without_time(model.clone(), &ThreadConfig::default());
        let trace = Arc::new(Mutex::new(Vec::new()));
        let mut probe = Probe::new("prompt", trace);
        probe.prompt_suffix = Some(" appended");
        h.agent = h.agent.with_middleware(Arc::new(probe));

        let (result, events, _) = h.turn("go").await;
        result.unwrap();

        assert_eq!(
            model.requests.lock().unwrap()[0].system_prompt.as_deref(),
            Some(" appended")
        );
        let baseline = ScriptedModel::new(vec![resp(Some("ok"), vec![])]);
        let (_, baseline_events, _) = run(baseline, "go", &ThreadConfig::default()).await;
        assert_eq!(kinds(&events), kinds(&baseline_events));
    }

    #[tokio::test]
    async fn after_model_rewrites_message_history_and_completion() {
        let model = ScriptedModel::new(vec![resp(Some("hello"), vec![])]);
        let mut h = Harness::without_time(model, &ThreadConfig::default());
        let trace = Arc::new(Mutex::new(Vec::new()));
        let mut probe = Probe::new("uppercase", trace);
        probe.uppercase_response = true;
        h.agent = h.agent.with_middleware(Arc::new(probe));

        let (result, events, _) = h.turn("go").await;
        result.unwrap();
        assert!(events.iter().any(|event| matches!(
            &event.msg,
            EventMsg::AssistantMessage(message) if message.content.as_deref() == Some("HELLO")
        )));
        assert_eq!(
            h.history().last(),
            Some(&HistoryItem::assistant(Some("HELLO".into()), Vec::new()))
        );
        assert!(matches!(
            &events.last().unwrap().msg,
            EventMsg::TurnComplete(complete)
                if complete.last_agent_message.as_deref() == Some("HELLO")
        ));
    }

    #[tokio::test]
    async fn before_tool_rewrites_arguments_but_not_call_identity() {
        let call = tc("echo", serde_json::json!({"input":"original"}));
        let model = ScriptedModel::new(vec![
            resp(None, vec![call.clone()]),
            resp(Some("done"), vec![]),
        ]);
        let mut h = Harness::without_time(model, &ThreadConfig::default());
        let trace = Arc::new(Mutex::new(Vec::new()));
        let mut probe = Probe::new("rewrite", trace);
        probe.tool_input = Some("patched");
        probe.rekey_call = true;
        probe.rekey_result = true;
        h.agent = h.agent.with_middleware(Arc::new(probe));

        let (result, events, _) = h.turn("go").await;
        result.unwrap();
        let begin = events
            .iter()
            .find_map(|event| match &event.msg {
                EventMsg::ToolCallBegin(begin) => Some(begin),
                _ => None,
            })
            .unwrap();
        let end = events
            .iter()
            .find_map(|event| match &event.msg {
                EventMsg::ToolCallEnd(end) => Some(end),
                _ => None,
            })
            .unwrap();
        assert_eq!(begin.tool_call_id, call.id);
        assert_eq!(begin.tool_name, "echo");
        assert_eq!(begin.arguments["input"], "patched");
        assert_eq!(end.result.call_id, call.id);
        assert_eq!(end.result.tool_name, "echo");
        assert_eq!(end.result.output, "patched");
    }

    #[tokio::test]
    async fn after_tool_rewrites_result_in_next_model_history() {
        let model = ScriptedModel::new(vec![
            resp(
                None,
                vec![tc("echo", serde_json::json!({"input":"original"}))],
            ),
            resp(Some("done"), vec![]),
        ]);
        let mut h = Harness::without_time(model.clone(), &ThreadConfig::default());
        let trace = Arc::new(Mutex::new(Vec::new()));
        let mut probe = Probe::new("result", trace);
        probe.after_output = Some("rewritten");
        h.agent = h.agent.with_middleware(Arc::new(probe));

        let (result, events, _) = h.turn("go").await;
        result.unwrap();
        assert!(events.iter().any(|event| matches!(
            &event.msg,
            EventMsg::ToolCallEnd(end) if end.result.output == "rewritten"
        )));
        assert!(request_history(&model.requests.lock().unwrap()[1])
            .iter()
            .any(|item| matches!(item, HistoryItem::ToolResult(result) if result.output == "rewritten")));
    }

    #[tokio::test]
    async fn skipped_tool_stops_before_hooks_but_runs_all_after_hooks() {
        let call = tc("echo", serde_json::json!({"input":"original"}));
        let model = ScriptedModel::new(vec![
            resp(None, vec![call.clone()]),
            resp(Some("done"), vec![]),
        ]);
        let executions = Arc::new(AtomicUsize::new(0));
        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(CountingEchoTool(executions.clone())));
        let mut h = Harness::without_time(model.clone(), &ThreadConfig::default());
        let trace = Arc::new(Mutex::new(Vec::new()));
        let mut first = Probe::new("first", trace.clone());
        first.skip_output = Some("skipped");
        first.rekey_result = true;
        let second = Probe::new("second", trace.clone());
        h.agent = AgentLoop::new(model, Arc::new(tools))
            .with_middlewares([Arc::new(first) as Arc<dyn Middleware>, Arc::new(second)]);

        let (result, events, _) = h.turn("go").await;
        result.unwrap();
        let trace = trace.lock().unwrap();
        assert!(!trace.iter().any(|entry| entry == "second.before_tool"));
        assert!(trace.iter().any(|entry| entry == "second.after_tool"));
        let begin_count = events
            .iter()
            .filter(|event| matches!(event.msg, EventMsg::ToolCallBegin(_)))
            .count();
        let ends: Vec<_> = events
            .iter()
            .filter_map(|event| match &event.msg {
                EventMsg::ToolCallEnd(end) => Some(&end.result),
                _ => None,
            })
            .collect();
        assert_eq!(begin_count, ends.len());
        assert_eq!(ends[0].output, "skipped");
        assert_eq!(ends[0].status, ToolCallStatus::Completed);
        assert_eq!(ends[0].call_id, call.id);
        assert_eq!(ends[0].tool_name, "echo");
        assert_eq!(executions.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn unknown_tool_still_runs_after_tool_hooks() {
        let model = ScriptedModel::new(vec![
            resp(None, vec![tc("missing", serde_json::Value::Null)]),
            resp(Some("done"), vec![]),
        ]);
        let mut h = Harness::without_time(model, &ThreadConfig::default());
        let trace = Arc::new(Mutex::new(Vec::new()));
        h.agent = h
            .agent
            .with_middleware(Arc::new(Probe::new("probe", trace.clone())));

        h.turn("go").await.0.unwrap();

        assert!(trace
            .lock()
            .unwrap()
            .iter()
            .any(|entry| entry == "probe.after_tool"));
    }

    #[tokio::test]
    async fn multiple_middlewares_apply_in_order_and_unwind_in_reverse() {
        let model = ScriptedModel::new(vec![
            resp(
                None,
                vec![tc("echo", serde_json::json!({"input":"original"}))],
            ),
            resp(Some("done"), vec![]),
        ]);
        let mut h = Harness::without_time(model, &ThreadConfig::default());
        let trace = Arc::new(Mutex::new(Vec::new()));
        let mut first = Probe::new("A", trace.clone());
        first.tool_input = Some("a");
        let mut second = Probe::new("B", trace.clone());
        second.append_tool_input = Some("b");
        h.agent = h
            .agent
            .with_middlewares([Arc::new(first) as Arc<dyn Middleware>, Arc::new(second)]);

        let (result, events, _) = h.turn("go").await;
        result.unwrap();

        let trace = trace.lock().unwrap();
        let calls_for = |node: &str| {
            trace
                .iter()
                .filter(|entry| entry.ends_with(node))
                .map(String::as_str)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            trace
                .iter()
                .filter(|entry| entry.ends_with("before_turn"))
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["A.before_turn", "B.before_turn"]
        );
        assert_eq!(
            trace
                .iter()
                .filter(|entry| entry.ends_with("after_turn"))
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["B.after_turn", "A.after_turn"]
        );
        assert_eq!(calls_for("before_tool"), ["A.before_tool", "B.before_tool"]);
        assert_eq!(
            calls_for("before_model"),
            [
                "A.before_model",
                "B.before_model",
                "A.before_model",
                "B.before_model"
            ]
        );
        for node in ["after_model", "after_tool"] {
            let calls = calls_for(node);
            if node == "after_model" {
                assert_eq!(
                    calls,
                    [
                        format!("B.{node}"),
                        format!("A.{node}"),
                        format!("B.{node}"),
                        format!("A.{node}")
                    ]
                );
            } else {
                assert_eq!(calls, [format!("B.{node}"), format!("A.{node}")]);
            }
        }
        assert!(events.iter().any(|event| matches!(
            &event.msg,
            EventMsg::ToolCallEnd(end) if end.result.output == "ab"
        )));
    }

    #[tokio::test]
    async fn before_model_error_stops_turn_without_after_turn_or_model_call() {
        let model = ScriptedModel::new(vec![resp(Some("unreachable"), vec![])]);
        let mut h = Harness::without_time(model.clone(), &ThreadConfig::default());
        let trace = Arc::new(Mutex::new(Vec::new()));
        let mut probe = Probe::new("failure", trace.clone());
        probe.fail_before_model = true;
        let next = Probe::new("next", trace.clone());
        h.agent = h
            .agent
            .with_middlewares([Arc::new(probe) as Arc<dyn Middleware>, Arc::new(next)]);

        let (result, events, _) = h.turn("go").await;

        assert!(matches!(result, Err(CoreError::Tool(_))));
        assert!(model.requests.lock().unwrap().is_empty());
        assert!(!events
            .iter()
            .any(|event| matches!(event.msg, EventMsg::TurnComplete(_))));
        assert!(!trace.lock().unwrap().iter().any(|entry| {
            entry == "failure.after_turn"
                || entry == "next.before_model"
                || entry == "next.after_turn"
        }));
    }

    #[tokio::test]
    async fn token_count_sums_usage_across_model_calls() {
        let usage = |input, output, read, write| TokenUsage {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: read,
            cache_write_tokens: write,
        };
        let reply = |usage, calls| {
            Ok(ModelResponse {
                content: None,
                tool_calls: calls,
                usage,
            })
        };
        let model = ScriptedModel::new(vec![
            reply(usage(10, 2, 5, 1), vec![tc("echo", Value::Null)]),
            reply(usage(20, 3, 7, 0), vec![]),
        ]);
        let (result, events, _) = run(model, "go", &ThreadConfig::default()).await;
        result.unwrap();

        let completed: Vec<_> = events
            .iter()
            .filter_map(|e| match &e.msg {
                EventMsg::ModelRequestCompleted(m) => Some(m.usage),
                _ => None,
            })
            .collect();
        assert_eq!(completed, [usage(10, 2, 5, 1), usage(20, 3, 7, 0)]);
        let total = events
            .iter()
            .find_map(|e| match &e.msg {
                EventMsg::TokenCount(t) => Some(t.usage),
                _ => None,
            })
            .unwrap();
        assert_eq!(total, usage(30, 5, 12, 1));
        assert_eq!(total.total_tokens(), 48);
    }
}
