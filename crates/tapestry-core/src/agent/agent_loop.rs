use std::sync::Arc;

use tapestry_protocol::{
    EnvironmentContextUpdatedEvent, EventMsg, TurnId, TurnStartedEvent, UserMessageEvent,
};

use crate::clock::{Clock, SystemClock};
use crate::config::ThreadConfig;
use crate::context::next_context_entry;
use crate::error::CoreError;
use crate::middleware::Middleware;
use crate::model::ModelClient;
use crate::recorder::Recorder;
use crate::tool::ToolRegistry;

use super::{Agent, AgentDeps, AgentFactory, ReactAgent, TurnContext};

/// Starts turns: records the turn prologue, then hands the turn to a fresh
/// per-turn [`Agent`] built from the configured model, tools and middleware.
pub struct AgentLoop {
    deps: AgentDeps,
    clock: Arc<dyn Clock>,
    agent_factory: AgentFactory,
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
            deps: AgentDeps::new(model, tools),
            clock,
            agent_factory: Arc::new(|deps, config| {
                Box::new(ReactAgent::new(deps, config.max_tool_iterations))
            }),
        }
    }

    pub fn with_agent_factory(
        mut self,
        factory: impl Fn(AgentDeps, &ThreadConfig) -> Box<dyn Agent> + Send + Sync + 'static,
    ) -> Self {
        self.agent_factory = Arc::new(factory);
        self
    }

    pub fn with_middleware(mut self, middleware: Arc<dyn Middleware>) -> Self {
        self.deps.push_middleware(middleware);
        self
    }

    pub fn with_middlewares(
        mut self,
        middlewares: impl IntoIterator<Item = Arc<dyn Middleware>>,
    ) -> Self {
        self.deps = self.deps.with_middlewares(middlewares);
        self
    }

    /// Run one turn, recording its events (and so its history) through
    /// `recorder`, whose events are stamped with `turn_id` from here on.
    ///
    /// Records `TurnStarted`, the optional environment context and the user
    /// message, then runs [`Agent::forward_with_hooks`].
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
        if let Some(context) = next_context_entry(
            &recorder.messages(),
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

        let mut agent = (self.agent_factory)(self.deps.clone(), config);
        let mut ctx = TurnContext::new(recorder, config, turn_id);
        agent.forward_with_hooks(&mut ctx).await
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
        ConversationItem, EnvironmentContext, Event, FinishReason, MessageId, ModelRequest,
        ModelResponse, ModelStreamItem, SubmissionId, SystemReminder, ThreadId, TokenUsage,
        ToolCall, ToolCallId, ToolCallStatus, ToolResult, ToolSpec,
    };

    use super::*;
    use crate::agent::{ReasoningOutput, StepOutput, TurnContext, TurnOutcome, TurnStep};
    use crate::clock::FixedClock;
    use crate::formatter::formatter_trait::ConversationProjection;
    use crate::middleware::{HookCtx, ToolDecision};
    use crate::model::{MockModelClient, ModelStream};
    use crate::store::testing::FlakyStore;
    use crate::store::{EventStore, InMemoryEventStore};
    use crate::tool::{EchoTool, Tool};

    fn request_history(request: &ModelRequest) -> Vec<ConversationItem> {
        ConversationProjection::fold(&request.events).messages()
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
        before_model_reminder: Option<&'static str>,
        after_model_reminder: Option<&'static str>,
        before_tool_reminder: Option<&'static str>,
        after_tool_reminder: Option<&'static str>,
        after_step_reminder: Option<&'static str>,
        seen_events: Option<Arc<Mutex<Vec<Vec<Event>>>>>,
        tool_input: Option<&'static str>,
        append_tool_input: Option<&'static str>,
        skip_output: Option<&'static str>,
        rekey_call: bool,
        after_output: Option<&'static str>,
        append_after_output: Option<&'static str>,
        rekey_result: bool,
        fail_before_model: bool,
        tool_count: Option<Arc<AtomicUsize>>,
        event_store: Option<Arc<dyn EventStore>>,
        turn_complete_seen: Option<Arc<AtomicBool>>,
        step_override: Option<TurnStep>,
        initial_step: Option<TurnStep>,
        fail_after_step: bool,
    }

    impl Probe {
        fn new(label: &'static str, trace: Arc<Mutex<Vec<String>>>) -> Self {
            Self {
                label,
                trace,
                before_model_reminder: None,
                after_model_reminder: None,
                before_tool_reminder: None,
                after_tool_reminder: None,
                after_step_reminder: None,
                seen_events: None,
                tool_input: None,
                append_tool_input: None,
                skip_output: None,
                rekey_call: false,
                after_output: None,
                append_after_output: None,
                rekey_result: false,
                fail_before_model: false,
                tool_count: None,
                event_store: None,
                turn_complete_seen: None,
                step_override: None,
                initial_step: None,
                fail_after_step: false,
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
        async fn before_turn(
            &self,
            _ctx: &HookCtx<'_>,
            initial_step: &mut TurnStep,
        ) -> Result<(), CoreError> {
            self.record("before_turn");
            if let Some(step) = &self.initial_step {
                *initial_step = step.clone();
            }
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

        async fn before_model(&self, ctx: &HookCtx<'_>, events: &[Event]) -> Result<(), CoreError> {
            self.record("before_model");
            if self.fail_before_model {
                return Err(CoreError::Tool("middleware before_model failed".into()));
            }
            if let Some(seen) = &self.seen_events {
                seen.lock().unwrap().push(events.to_vec());
            }
            if let Some(text) = self.before_model_reminder {
                ctx.remind(SystemReminder::custom(text));
            }
            Ok(())
        }

        async fn after_model(
            &self,
            ctx: &HookCtx<'_>,
            _response: &ModelResponse,
        ) -> Result<(), CoreError> {
            self.record("after_model");
            if let Some(text) = self.after_model_reminder {
                ctx.remind(SystemReminder::custom(text));
            }
            Ok(())
        }

        async fn before_tool(
            &self,
            ctx: &HookCtx<'_>,
            call: &mut ToolCall,
        ) -> Result<ToolDecision, CoreError> {
            self.record("before_tool");
            if let Some(text) = self.before_tool_reminder {
                ctx.remind(SystemReminder::custom(text));
            }
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
            ctx: &HookCtx<'_>,
            _call: &ToolCall,
            result: &mut ToolResult,
        ) -> Result<(), CoreError> {
            self.record("after_tool");
            if let Some(text) = self.after_tool_reminder {
                ctx.remind(SystemReminder::custom(text));
            }
            if let Some(output) = self.after_output {
                result.output = output.into();
            }
            if let Some(suffix) = self.append_after_output {
                result.output.push_str(suffix);
            }
            if self.rekey_result {
                result.call_id = ToolCallId::from("middleware-result-id");
                result.tool_name = "middleware-result-tool".into();
            }
            Ok(())
        }

        async fn after_step(
            &self,
            ctx: &HookCtx<'_>,
            _output: &StepOutput<'_>,
            next_step: &mut TurnStep,
        ) -> Result<(), CoreError> {
            self.record("after_step");
            if let Some(text) = self.after_step_reminder {
                ctx.remind(SystemReminder::custom(text));
            }
            if self.fail_after_step {
                return Err(CoreError::Tool("middleware after_step failed".into()));
            }
            if let Some(step) = &self.step_override {
                *next_step = step.clone();
            }
            Ok(())
        }
    }

    struct FinishAfterTools {
        trace: Arc<Mutex<Vec<String>>>,
        results: Arc<Mutex<Vec<ToolResult>>>,
    }

    struct SelectModelForToolStep;

    #[async_trait]
    impl Middleware for SelectModelForToolStep {
        async fn after_step(
            &self,
            _ctx: &HookCtx<'_>,
            output: &StepOutput<'_>,
            next_step: &mut TurnStep,
        ) -> Result<(), CoreError> {
            if matches!(output, StepOutput::Model(response) if !response.tool_calls.is_empty()) {
                *next_step = TurnStep::CallModel { allow_tools: true };
            }
            Ok(())
        }
    }

    struct RestoreToolStep;

    #[async_trait]
    impl Middleware for RestoreToolStep {
        async fn after_step(
            &self,
            _ctx: &HookCtx<'_>,
            output: &StepOutput<'_>,
            next_step: &mut TurnStep,
        ) -> Result<(), CoreError> {
            if matches!(output, StepOutput::Model(response) if !response.tool_calls.is_empty())
                && matches!(next_step, TurnStep::CallModel { .. })
            {
                *next_step = TurnStep::CallTool;
            }
            Ok(())
        }
    }

    struct DropCalls {
        next_step: TurnStep,
    }

    #[async_trait]
    impl Middleware for DropCalls {
        async fn after_step(
            &self,
            _ctx: &HookCtx<'_>,
            output: &StepOutput<'_>,
            next_step: &mut TurnStep,
        ) -> Result<(), CoreError> {
            if matches!(output, StepOutput::Model(response) if !response.tool_calls.is_empty()) {
                *next_step = self.next_step.clone();
            }
            Ok(())
        }
    }

    type IterationLog = Arc<Mutex<Vec<(&'static str, Option<u32>)>>>;

    struct IterationCapture(IterationLog);

    #[async_trait]
    impl Middleware for IterationCapture {
        async fn before_model(
            &self,
            ctx: &HookCtx<'_>,
            _events: &[Event],
        ) -> Result<(), CoreError> {
            self.0.lock().unwrap().push(("before_model", ctx.iteration));
            Ok(())
        }

        async fn after_model(
            &self,
            ctx: &HookCtx<'_>,
            _response: &ModelResponse,
        ) -> Result<(), CoreError> {
            self.0.lock().unwrap().push(("after_model", ctx.iteration));
            Ok(())
        }

        async fn before_tool(
            &self,
            ctx: &HookCtx<'_>,
            _call: &mut ToolCall,
        ) -> Result<ToolDecision, CoreError> {
            self.0.lock().unwrap().push(("before_tool", ctx.iteration));
            Ok(ToolDecision::Continue)
        }

        async fn after_tool(
            &self,
            ctx: &HookCtx<'_>,
            _call: &ToolCall,
            _result: &mut ToolResult,
        ) -> Result<(), CoreError> {
            self.0.lock().unwrap().push(("after_tool", ctx.iteration));
            Ok(())
        }

        async fn after_step(
            &self,
            ctx: &HookCtx<'_>,
            output: &StepOutput<'_>,
            _next_step: &mut TurnStep,
        ) -> Result<(), CoreError> {
            let step = match output {
                StepOutput::Model(_) => "after_step_model",
                StepOutput::Tools(_) => "after_step_tools",
            };
            self.0.lock().unwrap().push((step, ctx.iteration));
            Ok(())
        }
    }

    struct SkipFirstCall(AtomicUsize);

    #[async_trait]
    impl Middleware for SkipFirstCall {
        async fn before_tool(
            &self,
            _ctx: &HookCtx<'_>,
            call: &mut ToolCall,
        ) -> Result<ToolDecision, CoreError> {
            if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(ToolDecision::Skip(ToolResult::completed(call, "skipped")))
            } else {
                Ok(ToolDecision::Continue)
            }
        }
    }

    struct OverriddenStepsAgent {
        deps: AgentDeps,
        reasoning_calls: Arc<AtomicUsize>,
        acting_calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Agent for OverriddenStepsAgent {
        fn deps(&self) -> &AgentDeps {
            &self.deps
        }

        async fn reasoning(
            &self,
            _ctx: &mut TurnContext<'_>,
            request: ModelRequest,
        ) -> Result<ReasoningOutput, CoreError> {
            self.reasoning_calls.fetch_add(1, Ordering::SeqCst);
            let (content, tool_calls) = if request.iteration == 0 {
                (
                    None,
                    vec![tc("echo", serde_json::json!({"input":"ignored"}))],
                )
            } else {
                (Some("custom answer".into()), Vec::new())
            };
            Ok(ReasoningOutput {
                response: ModelResponse {
                    content,
                    tool_calls,
                    usage: TokenUsage {
                        input_tokens: 1,
                        output_tokens: 1,
                        ..Default::default()
                    },
                },
                streamed: None,
            })
        }

        async fn acting(&self, call: &ToolCall) -> ToolResult {
            self.acting_calls.fetch_add(1, Ordering::SeqCst);
            ToolResult::completed(call, "overridden tool output")
        }
    }

    struct OverriddenForwardAgent(AgentDeps);

    #[async_trait]
    impl Agent for OverriddenForwardAgent {
        fn deps(&self) -> &AgentDeps {
            &self.0
        }

        async fn forward(&mut self, _ctx: &mut TurnContext<'_>) -> Result<TurnOutcome, CoreError> {
            Ok(TurnOutcome {
                finish: FinishReason::Completed,
                last_agent_message: Some("custom flow".into()),
            })
        }
    }

    #[async_trait]
    impl Middleware for FinishAfterTools {
        async fn after_step(
            &self,
            _ctx: &HookCtx<'_>,
            output: &StepOutput<'_>,
            next_step: &mut TurnStep,
        ) -> Result<(), CoreError> {
            match output {
                StepOutput::Model(_) => self.trace.lock().unwrap().push("finish.model".into()),
                StepOutput::Tools(results) => {
                    self.trace.lock().unwrap().push("finish.tools".into());
                    self.results.lock().unwrap().extend_from_slice(results);
                    *next_step = TurnStep::Done {
                        finish: FinishReason::Completed,
                        last_agent_message: Some("tools are enough".into()),
                    };
                }
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

        fn history(&self) -> Vec<ConversationItem> {
            self.recorder.messages()
        }

        async fn folded(&self) -> Vec<ConversationItem> {
            ConversationProjection::fold(&self.log().await).messages()
        }

        fn context_entries(&self) -> Vec<EnvironmentContext> {
            self.history()
                .into_iter()
                .filter_map(|i| match i {
                    ConversationItem::EnvironmentContext(context) => Some(context),
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

    fn assert_history_valid(history: &[ConversationItem]) {
        for item in history {
            if let ConversationItem::Assistant { tool_calls, .. } = item {
                for call in tool_calls {
                    assert!(
                        history.iter().any(
                            |i| matches!(i, ConversationItem::ToolResult(r) if r.call_id == call.id)
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
        assert!(matches!(history[0], ConversationItem::User { .. }));
        assert!(
            matches!(&history[1], ConversationItem::Assistant { tool_calls, .. } if tool_calls.len() == 1)
        );
        assert!(matches!(history[2], ConversationItem::ToolResult { .. }));
        assert!(matches!(
            &history[3],
            ConversationItem::Assistant {
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
            Some(&ConversationItem::assistant(Some("Hi!".into()), vec![]))
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
        assert_eq!(h.history(), [ConversationItem::user(None, "go")]);
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

        // The reminder is recorded right after the last tool request completes.
        let k = kinds(&events);
        let at = k.iter().position(|s| s == "req_start:2").unwrap();
        assert_eq!(k[at - 1], "reminder");
        assert_eq!(k[at - 2], "req_done:1");
        assert_eq!(k.iter().filter(|s| *s == "reminder").count(), 1);
        match &events[at - 1].msg {
            EventMsg::SystemReminder(r) => {
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
            Some(&ConversationItem::system_reminder(
                SystemReminder::ToolLimitReached
            ))
        );
        assert_eq!(forced_history.len(), 6);
        // The finished request consumed the reminder; it stays only in the log.
        let history = h.history();
        assert_eq!(history.len(), 6);
        assert!(!history
            .iter()
            .any(|i| matches!(i, ConversationItem::SystemReminder(_))));
        assert!(matches!(
            &history[5],
            ConversationItem::Assistant { content: Some(c), .. } if c == "summary"
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
            Some(ConversationItem::Assistant { tool_calls, .. }) if tool_calls.is_empty()
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
            .filter(|i| matches!(i, ConversationItem::ToolResult { .. }))
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
            (
                ConversationItem::EnvironmentContext(ctx),
                ConversationItem::User { content: msg, .. },
            ) => {
                assert_eq!(ctx, &available("2026-11-04", "Asia/Shanghai"));
                assert_eq!(msg, "hello");
            }
            other => panic!("unexpected history: {other:?}"),
        }
        let request = t.model.requests.lock().unwrap()[0].clone();
        let sent = request_history(&request);
        assert!(matches!(&sent[0], ConversationItem::EnvironmentContext(_)));

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
        assert_eq!(entries[1], available("2026-11-05", "Asia/Shanghai"));
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
        assert_eq!(entries[1], EnvironmentContext::Unavailable);
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
        assert_eq!(entries[1], available("2026-11-04", "UTC"));
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
            ConversationItem::ToolResult(r) if r.call_id == call.id && r.tool_name == "miskeyed"
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
                "one.after_step",
                "one.before_tool",
                "one.after_tool",
                "one.after_step",
                "one.before_model",
                "one.after_model",
                "one.after_step",
                "one.after_turn"
            ]
        );
        assert!(!turn_complete_seen.load(Ordering::SeqCst));
    }

    fn reminder_texts(events: &[Event]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match &event.msg {
                EventMsg::SystemReminder(r) => Some(format!("{:?}", r.reminder)),
                _ => None,
            })
            .collect()
    }

    fn custom(text: &str) -> String {
        format!("{:?}", SystemReminder::custom(text))
    }

    fn last_message_text(model: &ScriptedModel, request: usize) -> String {
        let requests = model.requests.lock().unwrap();
        let items = request_history(&requests[request]);
        match items.last().unwrap() {
            ConversationItem::SystemReminder(SystemReminder::Custom { text }) => text.clone(),
            ConversationItem::SystemReminder(SystemReminder::ToolLimitReached) => {
                "tool_limit_reached".into()
            }
            other => format!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn model_hooks_observe_request_and_response_without_emitting_events() {
        let model = ScriptedModel::new(vec![resp(Some("ok"), vec![])]);
        let mut h = Harness::without_time(model.clone(), &ThreadConfig::default());
        let trace = Arc::new(Mutex::new(Vec::new()));
        let mut probe = Probe::new("observe", trace);
        let seen = Arc::new(Mutex::new(Vec::new()));
        probe.seen_events = Some(seen.clone());
        h.agent = h.agent.with_middleware(Arc::new(probe));

        let (result, events, _) = h.turn("go").await;
        result.unwrap();

        {
            let requests = model.requests.lock().unwrap();
            assert_eq!(
                seen.lock().unwrap().as_slice(),
                [requests[0].events.clone()]
            );
        }
        let baseline = ScriptedModel::new(vec![resp(Some("ok"), vec![])]);
        let (_, baseline_events, _) = run(baseline, "go", &ThreadConfig::default()).await;
        assert_eq!(kinds(&events), kinds(&baseline_events));
    }

    #[tokio::test]
    async fn model_hooks_cannot_add_tool_calls() {
        let model = ScriptedModel::new(vec![resp(Some("hello"), vec![])]);
        let mut h = Harness::without_time(model, &ThreadConfig::default());
        let trace = Arc::new(Mutex::new(Vec::new()));
        h.agent = h
            .agent
            .with_middleware(Arc::new(Probe::new("observe", trace)));

        let (result, events, _) = h.turn("go").await;
        result.unwrap();
        assert!(!events
            .iter()
            .any(|event| matches!(event.msg, EventMsg::ToolCallBegin(_))));
        assert!(matches!(
            &events.last().unwrap().msg,
            EventMsg::TurnComplete(complete)
                if complete.finish_reason == FinishReason::Completed
                    && complete.last_agent_message.as_deref() == Some("hello")
        ));
    }

    #[tokio::test]
    async fn before_model_reminder_is_in_this_request_exactly_once() {
        let model = ScriptedModel::new(vec![resp(Some("ok"), vec![])]);
        let mut h = Harness::without_time(model.clone(), &ThreadConfig::default());
        let trace = Arc::new(Mutex::new(Vec::new()));
        let mut probe = Probe::new("brief", trace);
        probe.before_model_reminder = Some("be brief");
        h.agent = h.agent.with_middleware(Arc::new(probe));

        let (result, events, _) = h.turn("go").await;
        result.unwrap();

        assert_eq!(reminder_texts(&events), [custom("be brief")]);
        let requests = model.requests.lock().unwrap();
        assert_eq!(
            requests[0]
                .events
                .iter()
                .filter(|event| matches!(event.msg, EventMsg::SystemReminder(_)))
                .count(),
            1
        );
        assert_eq!(
            request_history(&requests[0]).last(),
            Some(&ConversationItem::system_reminder(SystemReminder::custom(
                "be brief"
            )))
        );
        drop(requests);
        // Consumed by the completed request.
        assert_eq!(h.history().len(), 2);
    }

    #[tokio::test]
    async fn multiple_middlewares_record_reminders_in_registration_order() {
        let model = ScriptedModel::new(vec![resp(Some("ok"), vec![])]);
        let mut h = Harness::without_time(model.clone(), &ThreadConfig::default());
        let trace = Arc::new(Mutex::new(Vec::new()));
        let mut a = Probe::new("a", trace.clone());
        a.before_model_reminder = Some("a");
        let mut b = Probe::new("b", trace);
        b.before_model_reminder = Some("b");
        h.agent = h
            .agent
            .with_middlewares([Arc::new(a) as Arc<dyn Middleware>, Arc::new(b)]);

        let (result, events, _) = h.turn("go").await;
        result.unwrap();

        assert_eq!(reminder_texts(&events), [custom("a"), custom("b")]);
        let requests = model.requests.lock().unwrap();
        let items = request_history(&requests[0]);
        assert_eq!(
            items[items.len() - 2..],
            [
                ConversationItem::system_reminder(SystemReminder::custom("a")),
                ConversationItem::system_reminder(SystemReminder::custom("b")),
            ]
        );
    }

    #[tokio::test]
    async fn after_model_reminder_reaches_the_next_request_after_the_assistant_message() {
        let model = ScriptedModel::new(vec![
            resp(None, vec![tc("echo", serde_json::json!({"input":"x"}))]),
            resp(Some("done"), vec![]),
        ]);
        let mut h = Harness::without_time(model.clone(), &ThreadConfig::default());
        let trace = Arc::new(Mutex::new(Vec::new()));
        let mut probe = Probe::new("after", trace);
        probe.after_model_reminder = Some("after model");
        h.agent = h.agent.with_middleware(Arc::new(probe));

        let (result, events, _) = h.turn("go").await;
        result.unwrap();

        let k = kinds(&events);
        let assistant = k.iter().position(|s| s == "assistant:1").unwrap();
        assert!(k.iter().position(|s| s == "reminder").unwrap() > assistant);
        assert_eq!(last_message_text(&model, 1), "after model");
    }

    #[tokio::test]
    async fn tool_hook_reminders_never_split_a_tool_call_and_reach_the_next_request() {
        let model = ScriptedModel::new(vec![
            resp(
                None,
                vec![
                    tc("echo", serde_json::json!({"input":"1"})),
                    tc("echo", serde_json::json!({"input":"2"})),
                ],
            ),
            resp(Some("done"), vec![]),
        ]);
        let mut h = Harness::without_time(model.clone(), &ThreadConfig::default());
        let trace = Arc::new(Mutex::new(Vec::new()));
        let mut probe = Probe::new("tool", trace);
        probe.before_tool_reminder = Some("before tool");
        probe.after_tool_reminder = Some("after tool");
        h.agent = h.agent.with_middleware(Arc::new(probe));

        let (result, events, _) = h.turn("go").await;
        result.unwrap();

        let k = kinds(&events);
        let mut open = false;
        for kind in &k {
            match kind.as_str() {
                "tool_begin" => open = true,
                "reminder" => assert!(!open, "reminder inside a tool call: {k:?}"),
                kind if kind.starts_with("tool_end") => open = false,
                _ => {}
            }
        }
        assert_eq!(k.iter().filter(|s| *s == "reminder").count(), 4);
        let last_tool_end = k.iter().rposition(|s| s.starts_with("tool_end")).unwrap();
        assert!(
            k.iter().position(|s| s == "reminder").unwrap()
                > k.iter().position(|s| s == "assistant:2").unwrap()
        );
        assert!(k.iter().rposition(|s| s == "reminder").unwrap() > last_tool_end);

        let requests = model.requests.lock().unwrap();
        let items = request_history(&requests[1]);
        assert!(matches!(
            items[items.len() - 5],
            ConversationItem::ToolResult(_)
        ));
        assert_eq!(
            items[items.len() - 4..]
                .iter()
                .filter(|item| matches!(item, ConversationItem::SystemReminder(_)))
                .count(),
            4
        );
    }

    #[tokio::test]
    async fn after_step_reminder_before_done_is_recorded_before_token_count() {
        let model = ScriptedModel::new(vec![resp(Some("ok"), vec![]), resp(Some("again"), vec![])]);
        let mut h = Harness::without_time(model.clone(), &ThreadConfig::default());
        let trace = Arc::new(Mutex::new(Vec::new()));
        let mut probe = Probe::new("step", trace);
        probe.after_step_reminder = Some("wrap up");
        h.agent = h.agent.with_middleware(Arc::new(probe));

        let (result, events, _) = h.turn("go").await;
        result.unwrap();

        let k = kinds(&events);
        let reminder = k.iter().position(|s| s == "reminder").unwrap();
        let token_count = k.iter().position(|s| s == "token_count").unwrap();
        assert!(reminder < token_count);
        // The next turn never sees it.
        let (result, _, _) = h.turn("next").await;
        result.unwrap();
        let requests = model.requests.lock().unwrap();
        assert!(!request_history(&requests[1])
            .iter()
            .any(|item| matches!(item, ConversationItem::SystemReminder(_))));
    }

    #[tokio::test]
    async fn after_turn_reminder_is_recorded_before_token_count() {
        struct AfterTurnReminder;

        #[async_trait]
        impl Middleware for AfterTurnReminder {
            async fn after_turn(&self, ctx: &HookCtx<'_>) -> Result<(), CoreError> {
                ctx.remind(SystemReminder::custom("bye"));
                Ok(())
            }
        }

        let model = ScriptedModel::new(vec![resp(Some("ok"), vec![])]);
        let mut h = Harness::without_time(model, &ThreadConfig::default());
        h.agent = h.agent.with_middleware(Arc::new(AfterTurnReminder));
        let (result, events, _) = h.turn("go").await;
        result.unwrap();
        let k = kinds(&events);
        assert_eq!(
            k[k.len() - 3..],
            ["reminder", "token_count", "turn_complete"]
        );
    }

    #[tokio::test]
    async fn before_turn_reminder_is_delivered_to_the_first_request() {
        struct BeforeTurnReminder;

        #[async_trait]
        impl Middleware for BeforeTurnReminder {
            async fn before_turn(
                &self,
                ctx: &HookCtx<'_>,
                _initial_step: &mut TurnStep,
            ) -> Result<(), CoreError> {
                ctx.remind(SystemReminder::custom("start"));
                Ok(())
            }
        }

        let model = ScriptedModel::new(vec![resp(Some("ok"), vec![])]);
        let mut h = Harness::without_time(model.clone(), &ThreadConfig::default());
        h.agent = h.agent.with_middleware(Arc::new(BeforeTurnReminder));
        let (result, events, _) = h.turn("go").await;
        result.unwrap();
        assert_eq!(reminder_texts(&events), [custom("start")]);
        assert_eq!(last_message_text(&model, 0), "start");
    }

    #[tokio::test]
    async fn reminder_record_failure_ends_turn_without_a_model_request() {
        // turn_started, user, req_start, then the reminder append fails.
        let store = Arc::new(FlakyStore::default());
        store.fail_after(3);
        let model = ScriptedModel::new(vec![resp(Some("ok"), vec![])]);
        let mut h = Harness::with_store(
            model.clone(),
            Arc::new(FixedClock::new("2026-11-04", "UTC")),
            ThreadConfig {
                include_time_context: false,
                ..Default::default()
            },
            store,
        );
        let trace = Arc::new(Mutex::new(Vec::new()));
        let mut probe = Probe::new("brief", trace);
        probe.before_model_reminder = Some("be brief");
        h.agent = h.agent.with_middleware(Arc::new(probe));

        let (result, events, _) = h.turn("go").await;

        assert!(matches!(result, Err(CoreError::EventStore(_))));
        assert!(model.requests.lock().unwrap().is_empty());
        assert!(!events
            .iter()
            .any(|event| matches!(event.msg, EventMsg::TurnComplete(_))));
    }

    #[tokio::test]
    async fn zero_tool_budget_request_ends_with_the_limit_reminder() {
        let model = ScriptedModel::new(vec![resp(Some("summary"), vec![])]);
        let config = ThreadConfig {
            max_tool_iterations: 0,
            ..Default::default()
        };
        let (result, events, _) = run(model.clone(), "go", &config).await;
        result.unwrap();
        assert!(model.requests.lock().unwrap()[0].tools.is_empty());
        assert_eq!(last_message_text(&model, 0), "tool_limit_reached");
        let k = kinds(&events);
        let reminder = k.iter().position(|s| s == "reminder").unwrap();
        assert!(reminder < k.iter().position(|s| s == "req_start:0").unwrap());
    }

    #[tokio::test]
    async fn limit_reminder_does_not_interleave_with_assistant_message_events() {
        let echo = || tc("echo", serde_json::json!({"input": "x"}));
        let model = ScriptedModel::new(vec![resp(None, vec![echo()]), resp(Some("end"), vec![])]);
        let config = ThreadConfig {
            max_tool_iterations: 1,
            ..Default::default()
        };
        let (result, events, _) = run(model.clone(), "go", &config).await;
        result.unwrap();
        let k = kinds(&events);
        let reminder = k.iter().position(|s| s == "reminder").unwrap();
        assert_eq!(k[reminder - 1], "req_done:0");
        assert_eq!(k[reminder + 1], "req_start:1");
        assert_eq!(last_message_text(&model, 1), "tool_limit_reached");
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
            .any(|item| matches!(item, ConversationItem::ToolResult(result) if result.output == "rewritten")));
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
    async fn skipped_first_call_does_not_skip_the_rest_of_the_batch() {
        let calls = vec![
            tc("echo", serde_json::json!({"input":"skipped"})),
            tc("echo", serde_json::json!({"input":"executed"})),
        ];
        let model = ScriptedModel::new(vec![resp(None, calls), resp(Some("done"), vec![])]);
        let executions = Arc::new(AtomicUsize::new(0));
        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(CountingEchoTool(executions.clone())));
        let mut h = Harness::without_time(model.clone(), &ThreadConfig::default());
        h.agent = AgentLoop::new(model, Arc::new(tools))
            .with_middleware(Arc::new(SkipFirstCall(AtomicUsize::new(0))));

        let (result, events, _) = h.turn("go").await;

        result.unwrap();
        assert_eq!(executions.load(Ordering::SeqCst), 1);
        let results = events
            .iter()
            .filter_map(|event| match &event.msg {
                EventMsg::ToolCallEnd(end) => Some(end.result.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].output, "skipped");
        assert_eq!(results[1].output, "executed");
        assert!(results
            .iter()
            .all(|result| result.status == ToolCallStatus::Completed));
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
    async fn multiple_middlewares_apply_in_registration_order() {
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
        first.after_output = Some("a");
        let mut second = Probe::new("B", trace.clone());
        second.append_tool_input = Some("b");
        second.append_after_output = Some("b");
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
            ["A.after_turn", "B.after_turn"]
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
        for node in ["after_model", "after_tool", "after_step"] {
            let calls = calls_for(node);
            let count = match node {
                "after_tool" => 1,
                "after_step" => 3,
                _ => 2,
            };
            let expected = (0..count)
                .flat_map(|_| [format!("A.{node}"), format!("B.{node}")])
                .collect::<Vec<_>>();
            assert_eq!(calls, expected);
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

    #[tokio::test]
    async fn after_step_can_finish_after_ordered_tool_calls() {
        let calls = vec![
            tc("echo", serde_json::json!({"input":"first"})),
            tc("echo", serde_json::json!({"input":"second"})),
        ];
        let model = ScriptedModel::new(vec![resp(None, calls)]);
        let mut h = Harness::without_time(model.clone(), &ThreadConfig::default());
        let trace = Arc::new(Mutex::new(Vec::new()));
        let results = Arc::new(Mutex::new(Vec::new()));
        h.agent = h.agent.with_middleware(Arc::new(FinishAfterTools {
            trace: trace.clone(),
            results: results.clone(),
        }));

        let (result, events, _) = h.turn("go").await;

        result.unwrap();
        assert_eq!(model.requests.lock().unwrap().len(), 1);
        assert_eq!(*trace.lock().unwrap(), ["finish.model", "finish.tools"]);
        let received = results.lock().unwrap();
        assert_eq!(received.len(), 2);
        assert_eq!(received[0].output, "first");
        assert_eq!(received[1].output, "second");
        assert_eq!(
            kinds(&events),
            [
                "turn_started",
                "user",
                "req_start:0",
                "start",
                "assistant:2",
                "tool_begin",
                "tool_end:Completed",
                "tool_begin",
                "tool_end:Completed",
                "req_done:0",
                "token_count",
                "turn_complete"
            ]
        );
        let completed_usage = events.iter().find_map(|event| match &event.msg {
            EventMsg::ModelRequestCompleted(completed) => Some(completed.usage),
            _ => None,
        });
        let total_usage = events.iter().find_map(|event| match &event.msg {
            EventMsg::TokenCount(total) => Some(total.usage),
            _ => None,
        });
        assert_eq!(completed_usage, total_usage);
        assert_eq!(
            total_usage,
            Some(TokenUsage {
                input_tokens: 1,
                output_tokens: 2,
                ..Default::default()
            })
        );
        assert!(matches!(
            &events.last().unwrap().msg,
            EventMsg::TurnComplete(complete)
                if complete.finish_reason == FinishReason::Completed
                    && complete.last_agent_message.as_deref() == Some("tools are enough")
        ));
    }

    #[tokio::test]
    async fn later_middleware_can_replace_a_nonterminal_plan() {
        let model = ScriptedModel::new(vec![
            resp(None, vec![tc("echo", serde_json::json!({"input":"once"}))]),
            resp(Some("done"), vec![]),
        ]);
        let mut h = Harness::without_time(model.clone(), &ThreadConfig::default());
        h.agent = h.agent.with_middlewares([
            Arc::new(SelectModelForToolStep) as Arc<dyn Middleware>,
            Arc::new(RestoreToolStep),
        ]);

        let (result, events, _) = h.turn("go").await;

        result.unwrap();
        assert_eq!(model.requests.lock().unwrap().len(), 2);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.msg, EventMsg::ToolCallBegin(_)))
                .count(),
            1
        );
        assert!(!events
            .iter()
            .any(|event| matches!(event.msg, EventMsg::Warning(_))));
    }

    #[tokio::test]
    async fn model_and_its_tool_calls_share_one_iteration_number() {
        let model = ScriptedModel::new(vec![
            resp(None, vec![tc("echo", serde_json::json!({"input":"one"}))]),
            resp(Some("done"), vec![]),
        ]);
        let mut h = Harness::without_time(model, &ThreadConfig::default());
        let iterations = Arc::new(Mutex::new(Vec::new()));
        h.agent = h
            .agent
            .with_middleware(Arc::new(IterationCapture(iterations.clone())));

        let (result, _, _) = h.turn("go").await;

        result.unwrap();
        assert_eq!(
            *iterations.lock().unwrap(),
            [
                ("before_model", Some(0)),
                ("after_model", Some(0)),
                ("after_step_model", Some(0)),
                ("before_tool", Some(0)),
                ("after_tool", Some(0)),
                ("after_step_tools", Some(0)),
                ("before_model", Some(1)),
                ("after_model", Some(1)),
                ("after_step_model", Some(1)),
            ]
        );
    }

    #[tokio::test]
    async fn unexecuted_model_calls_are_removed_before_assistant_message_commit() {
        let call = tc("echo", serde_json::json!({"input":"ignored"}));
        for (next_step, extra_response, expected_requests) in [
            (
                TurnStep::Done {
                    finish: FinishReason::Completed,
                    last_agent_message: Some("finished".into()),
                },
                None,
                1,
            ),
            (
                TurnStep::CallModel { allow_tools: true },
                Some(resp(Some("answer"), vec![])),
                2,
            ),
        ] {
            let mut replies = vec![resp(Some("partial"), vec![call.clone()])];
            if let Some(response) = extra_response {
                replies.push(response);
            }
            let model = ScriptedModel::new(replies);
            let mut h = Harness::without_time(model.clone(), &ThreadConfig::default());
            h.agent = h.agent.with_middleware(Arc::new(DropCalls { next_step }));

            let (result, events, _) = h.turn("go").await;

            result.unwrap();
            let first_assistant = events.iter().find_map(|event| match &event.msg {
                EventMsg::AssistantMessage(message) => Some(message),
                _ => None,
            });
            let first_assistant = first_assistant.unwrap();
            assert_eq!(first_assistant.content.as_deref(), Some("partial"));
            assert!(first_assistant.tool_calls.is_empty());
            assert!(events.iter().any(|event| matches!(
                &event.msg,
                EventMsg::Warning(warning) if warning.message.contains("ignored 1")
            )));
            assert!(!events
                .iter()
                .any(|event| matches!(event.msg, EventMsg::ToolCallBegin(_))));
            let requests = model.requests.lock().unwrap();
            assert_eq!(requests.len(), expected_requests);
            if requests.len() == 2 {
                let history = request_history(&requests[1]);
                assert!(matches!(
                    history.last(),
                    Some(ConversationItem::Assistant { tool_calls, content: Some(text) })
                        if tool_calls.is_empty() && text == "partial"
                ));
            }
        }
    }

    #[tokio::test]
    async fn a_locked_default_done_cannot_be_reopened() {
        let model = ScriptedModel::new(vec![resp(Some("answer"), vec![])]);
        let mut h = Harness::without_time(model.clone(), &ThreadConfig::default());
        let mut probe = Probe::new("reopen", Arc::new(Mutex::new(Vec::new())));
        probe.step_override = Some(TurnStep::CallModel { allow_tools: false });
        h.agent = h.agent.with_middleware(Arc::new(probe));

        let (result, events, _) = h.turn("go").await;

        assert!(
            matches!(result, Err(CoreError::Internal(message)) if message.contains("locked Done"))
        );
        assert_eq!(model.requests.lock().unwrap().len(), 1);
        assert!(!events
            .iter()
            .any(|event| matches!(event.msg, EventMsg::TurnComplete(_))));
    }

    #[tokio::test]
    async fn initial_done_skips_model_and_still_completes_the_turn() {
        let model = ScriptedModel::new(vec![]);
        let mut h = Harness::without_time(model.clone(), &ThreadConfig::default());
        let mut probe = Probe::new("initial", Arc::new(Mutex::new(Vec::new())));
        probe.initial_step = Some(TurnStep::Done {
            finish: FinishReason::Completed,
            last_agent_message: Some("no model needed".into()),
        });
        h.agent = h.agent.with_middleware(Arc::new(probe));

        let (result, events, _) = h.turn("go").await;

        result.unwrap();
        assert!(model.requests.lock().unwrap().is_empty());
        assert_eq!(
            kinds(&events),
            ["turn_started", "user", "token_count", "turn_complete"]
        );
        assert!(matches!(
            &events.last().unwrap().msg,
            EventMsg::TurnComplete(complete)
                if complete.last_agent_message.as_deref() == Some("no model needed")
        ));
    }

    #[tokio::test]
    async fn initial_call_tool_without_pending_tool_calls_is_an_error() {
        let model = ScriptedModel::new(vec![]);
        let mut h = Harness::without_time(model.clone(), &ThreadConfig::default());
        let mut probe = Probe::new("initial", Arc::new(Mutex::new(Vec::new())));
        probe.initial_step = Some(TurnStep::CallTool);
        h.agent = h.agent.with_middleware(Arc::new(probe));

        let (result, events, _) = h.turn("go").await;

        assert!(
            matches!(result, Err(CoreError::Internal(message)) if message.contains("pending tool calls"))
        );
        assert!(model.requests.lock().unwrap().is_empty());
        assert!(!events
            .iter()
            .any(|event| matches!(event.msg, EventMsg::TurnComplete(_))));
    }

    #[tokio::test]
    async fn after_step_error_stops_later_hooks_and_completion() {
        let model = ScriptedModel::new(vec![resp(Some("answer"), vec![])]);
        let mut h = Harness::without_time(model, &ThreadConfig::default());
        let trace = Arc::new(Mutex::new(Vec::new()));
        let mut failing = Probe::new("failing", trace.clone());
        failing.fail_after_step = true;
        let observer = Probe::new("observer", trace.clone());
        h.agent = h
            .agent
            .with_middlewares([Arc::new(failing) as Arc<dyn Middleware>, Arc::new(observer)]);

        let (result, events, _) = h.turn("go").await;

        assert!(matches!(result, Err(CoreError::Tool(message)) if message.contains("after_step")));
        assert_eq!(
            *trace.lock().unwrap(),
            [
                "failing.before_turn",
                "observer.before_turn",
                "failing.before_model",
                "observer.before_model",
                "failing.after_model",
                "observer.after_model",
                "failing.after_step"
            ]
        );
        assert!(!events
            .iter()
            .any(|event| matches!(event.msg, EventMsg::TurnComplete(_))));
    }

    #[tokio::test]
    async fn reasoning_and_single_tool_overrides_run_inside_their_hooks() {
        let model = ScriptedModel::new(vec![]);
        let mut h = Harness::without_time(model, &ThreadConfig::default());
        let reasoning_calls = Arc::new(AtomicUsize::new(0));
        let acting_calls = Arc::new(AtomicUsize::new(0));
        let trace = Arc::new(Mutex::new(Vec::new()));
        h.agent = h
            .agent
            .with_middleware(Arc::new(Probe::new("hooks", trace.clone())));
        let reasoning_count = reasoning_calls.clone();
        let acting_count = acting_calls.clone();
        h.agent = h.agent.with_agent_factory(move |deps, _| {
            Box::new(OverriddenStepsAgent {
                deps,
                reasoning_calls: reasoning_count.clone(),
                acting_calls: acting_count.clone(),
            })
        });

        let (result, events, _) = h.turn("go").await;

        result.unwrap();
        assert_eq!(reasoning_calls.load(Ordering::SeqCst), 2);
        assert_eq!(acting_calls.load(Ordering::SeqCst), 1);
        assert!(events.iter().any(|event| matches!(
            &event.msg,
            EventMsg::ToolCallEnd(end) if end.result.output == "overridden tool output"
        )));
        assert!(matches!(
            &events.last().unwrap().msg,
            EventMsg::TurnComplete(complete)
                if complete.last_agent_message.as_deref() == Some("custom answer")
        ));
        let trace = trace.lock().unwrap();
        assert_eq!(
            trace
                .iter()
                .filter(|item| item.ends_with("before_model"))
                .count(),
            2
        );
        assert_eq!(
            trace
                .iter()
                .filter(|item| item.ends_with("after_model"))
                .count(),
            2
        );
        assert_eq!(
            trace
                .iter()
                .filter(|item| item.ends_with("before_tool"))
                .count(),
            1
        );
        assert_eq!(
            trace
                .iter()
                .filter(|item| item.ends_with("after_tool"))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn forward_override_is_wrapped_and_completes_once() {
        let mut h = Harness::without_time(ScriptedModel::new(vec![]), &ThreadConfig::default());
        let trace = Arc::new(Mutex::new(Vec::new()));
        let mut probe = Probe::new("turn", trace.clone());
        let turn_complete_seen = Arc::new(AtomicBool::new(true));
        probe.event_store = Some(h.store.clone());
        probe.turn_complete_seen = Some(turn_complete_seen.clone());
        h.agent = h.agent.with_middleware(Arc::new(probe));
        h.agent = h
            .agent
            .with_agent_factory(|deps, _| Box::new(OverriddenForwardAgent(deps)));

        let (result, events, _) = h.turn("go").await;

        result.unwrap();
        assert_eq!(
            *trace.lock().unwrap(),
            ["turn.before_turn", "turn.after_turn"]
        );
        assert!(!turn_complete_seen.load(Ordering::SeqCst));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.msg, EventMsg::TokenCount(_)))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.msg, EventMsg::TurnComplete(_)))
                .count(),
            1
        );
        assert!(matches!(
            &events.last().unwrap().msg,
            EventMsg::TurnComplete(complete)
                if complete.last_agent_message.as_deref() == Some("custom flow")
        ));
    }

    #[tokio::test]
    #[cfg(any())]
    async fn policy_output_must_be_an_ordered_subsequence_and_tools_must_be_allowed() {
        let first = tc("echo", serde_json::json!({"input":"first"}));
        let second = tc("echo", serde_json::json!({"input":"second"}));
        let forged = tc("echo", serde_json::json!({"input":"forged"}));
        let cases = [
            (
                vec![first.clone(), second.clone()],
                vec![second.clone(), first.clone()],
                true,
            ),
            (vec![first.clone()], vec![forged], true),
            (vec![first.clone()], vec![first.clone()], false),
        ];

        for (requested, retained, allow_tools) in cases {
            let model = ScriptedModel::new(vec![resp(None, requested)]);
            let mut h = Harness::without_time(model, &ThreadConfig::default());
            h.agent = h.agent.with_agent_factory(move |deps, _| {
                Box::new(ScriptAgent::new(
                    deps,
                    vec![
                        TurnStep::CallModel {
                            allow_tools,
                            reminder: None,
                        },
                        TurnStep::Done {
                            finish: FinishReason::Completed,
                            last_agent_message: None,
                        },
                    ],
                    vec![retained.clone()],
                    None,
                ))
            });

            let (result, events, _) = h.turn("go").await;
            assert!(matches!(result, Err(CoreError::Internal(_))));
            assert!(!events
                .iter()
                .any(|event| matches!(event.msg, EventMsg::ToolCallBegin(_))));
            assert!(!events
                .iter()
                .any(|event| matches!(event.msg, EventMsg::TurnComplete(_))));
            assert!(!events
                .iter()
                .any(|event| matches!(event.msg, EventMsg::AssistantMessage(_))));
        }
    }

    #[tokio::test]
    #[cfg(any())]
    async fn policy_cannot_request_more_than_max_tool_iterations_plus_one_calls() {
        let max_tool_iterations = 2;
        let model = ScriptedModel::new(
            (0..=max_tool_iterations)
                .map(|_| resp(Some("again"), vec![]))
                .collect(),
        );
        let mut h = Harness::without_time(
            model.clone(),
            &ThreadConfig {
                max_tool_iterations,
                ..Default::default()
            },
        );
        h.agent = h
            .agent
            .with_agent_factory(|deps, _| Box::new(InfiniteAgent(deps)));

        let (result, events, _) = h.turn("go").await;
        assert!(matches!(result, Err(CoreError::Internal(_))));
        assert_eq!(
            model.requests.lock().unwrap().len(),
            (max_tool_iterations + 1) as usize
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.msg, EventMsg::ModelRequestStarted(_)))
                .count(),
            (max_tool_iterations + 1) as usize
        );
        assert!(!events
            .iter()
            .any(|event| matches!(event.msg, EventMsg::TurnComplete(_))));
    }

    #[tokio::test]
    #[cfg(any())]
    async fn custom_policy_can_continue_after_text_and_choose_finish_reason() {
        let model = ScriptedModel::new(vec![
            resp(Some("thinking"), vec![]),
            resp(Some("answer"), vec![]),
        ]);
        let mut h = Harness::without_time(model.clone(), &ThreadConfig::default());
        h.agent = h.agent.with_agent_factory(|deps, _| {
            Box::new(ScriptAgent::new(
                deps,
                vec![
                    TurnStep::CallModel { allow_tools: true },
                    TurnStep::CallModel {
                        allow_tools: false,
                        reminder: Some(SystemReminder::ToolLimitReached),
                    },
                    TurnStep::Done {
                        finish: FinishReason::ExceedMaxIters,
                        last_agent_message: Some("policy finished".into()),
                    },
                ],
                vec![vec![], vec![]],
                None,
            ))
        });

        let (result, events, _) = h.turn("go").await;
        result.unwrap();
        assert_eq!(
            kinds(&events),
            [
                "turn_started",
                "user",
                "req_start:0",
                "start",
                "assistant:0",
                "req_done:0",
                "req_start:1",
                "reminder",
                "start",
                "assistant:0",
                "req_done:1",
                "token_count",
                "turn_complete"
            ]
        );
        assert!(matches!(
            &events.last().unwrap().msg,
            EventMsg::TurnComplete(event)
                if event.finish_reason == FinishReason::ExceedMaxIters
                    && event.last_agent_message.as_deref() == Some("policy finished")
        ));
        let requests = model.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(!requests[0].tools.is_empty());
        assert!(requests[1].tools.is_empty());
    }

    #[tokio::test]
    #[cfg(any())]
    async fn custom_policy_keeps_second_call_warns_and_receives_paired_results() {
        let first = tc("echo", serde_json::json!({"input":"first"}));
        let second = tc("echo", serde_json::json!({"input":"second"}));
        let results = Arc::new(Mutex::new(Vec::new()));
        let model = ScriptedModel::new(vec![
            resp(None, vec![first.clone(), second.clone()]),
            resp(Some("done"), vec![]),
        ]);
        let mut h = Harness::without_time(model, &ThreadConfig::default());
        let captured = results.clone();
        let expected_second = second.clone();
        h.agent = h.agent.with_agent_factory(move |deps, _| {
            Box::new(ScriptAgent::new(
                deps,
                vec![
                    TurnStep::CallModel { allow_tools: true },
                    TurnStep::CallModel { allow_tools: true },
                    TurnStep::Done {
                        finish: FinishReason::Completed,
                        last_agent_message: Some("done".into()),
                    },
                ],
                vec![vec![second.clone()], vec![]],
                Some(captured.clone()),
            ))
        });

        let (result, events, _) = h.turn("go").await;
        result.unwrap();
        assert_eq!(
            kinds(&events),
            [
                "turn_started",
                "user",
                "req_start:0",
                "start",
                "assistant:1",
                "warning",
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
        let warning = events.iter().find_map(|event| match &event.msg {
            EventMsg::Warning(warning) => Some(&warning.message),
            _ => None,
        });
        assert!(warning.is_some_and(|message| message.contains("1")));
        let assistant = events.iter().find_map(|event| match &event.msg {
            EventMsg::AssistantMessage(message) => Some(message),
            _ => None,
        });
        let assistant_calls = &assistant.unwrap().tool_calls;
        assert_eq!(assistant_calls.len(), 1);
        assert_eq!(assistant_calls[0], expected_second);
        let paired_results = results.lock().unwrap();
        assert_eq!(paired_results.len(), 1);
        assert_eq!(paired_results[0].len(), 1);
        assert_eq!(paired_results[0][0].call_id, expected_second.id);
        assert_eq!(paired_results[0][0].tool_name, expected_second.name);
    }

    #[tokio::test]
    #[cfg(any())]
    async fn policy_receives_tool_results_in_call_order_with_matching_ids() {
        let calls = vec![
            tc("echo", serde_json::json!({"input":"first"})),
            tc("echo", serde_json::json!({"input":"second"})),
            tc("echo", serde_json::json!({"input":"third"})),
        ];
        let results = Arc::new(Mutex::new(Vec::new()));
        let model = ScriptedModel::new(vec![resp(None, calls.clone())]);
        let mut h = Harness::without_time(model, &ThreadConfig::default());
        let captured = results.clone();
        let expected_calls = calls.clone();
        h.agent = h.agent.with_agent_factory(move |deps, _| {
            Box::new(ScriptAgent::new(
                deps,
                vec![
                    TurnStep::CallModel { allow_tools: true },
                    TurnStep::Done {
                        finish: FinishReason::Completed,
                        last_agent_message: None,
                    },
                ],
                vec![calls.clone()],
                Some(captured.clone()),
            ))
        });

        h.turn("go").await.0.unwrap();
        let received = results.lock().unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(
            received[0]
                .iter()
                .map(|result| result.call_id.clone())
                .collect::<Vec<_>>(),
            expected_calls
                .iter()
                .map(|call| call.id.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            received[0]
                .iter()
                .map(|result| result.tool_name.as_str())
                .collect::<Vec<_>>(),
            expected_calls
                .iter()
                .map(|call| call.name.as_str())
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    #[cfg(any())]
    async fn factory_creates_a_fresh_policy_for_every_turn() {
        let model = ScriptedModel::new(vec![resp(Some("one"), vec![]), resp(Some("two"), vec![])]);
        let mut h = Harness::without_time(model, &ThreadConfig::default());
        let created = Arc::new(AtomicUsize::new(0));
        let factory_created = created.clone();
        h.agent = h.agent.with_agent_factory(move |deps, _| {
            factory_created.fetch_add(1, Ordering::SeqCst);
            Box::new(ScriptAgent::new(
                deps,
                vec![
                    TurnStep::CallModel { allow_tools: true },
                    TurnStep::Done {
                        finish: FinishReason::Completed,
                        last_agent_message: Some("fresh".into()),
                    },
                ],
                vec![vec![]],
                None,
            ))
        });

        assert!(h.turn("first").await.0.is_ok());
        assert!(h.turn("second").await.0.is_ok());
        assert_eq!(created.load(Ordering::SeqCst), 2);
    }
}
