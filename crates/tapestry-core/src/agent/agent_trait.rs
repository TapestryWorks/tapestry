use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tapestry_protocol::{
    AssistantMessageDeltaEvent, AssistantMessageEvent, AssistantMessageStartEvent, Event, EventMsg,
    FinishReason, MessageId, ModelRequest, ModelRequestCompletedEvent, ModelRequestStartedEvent,
    ModelResponse, ModelStreamItem, SystemReminder, SystemReminderEvent, TokenCountEvent,
    TokenUsage, ToolCall, ToolCallBeginEvent, ToolCallEndEvent, ToolResult, TurnCompleteEvent,
    TurnId, WarningEvent,
};

use crate::config::ThreadConfig;
use crate::error::CoreError;
use crate::middleware::{HookCtx, Middleware, ToolDecision};
use crate::model::ModelClient;
use crate::recorder::Recorder;
use crate::reminder::ReminderQueue;
use crate::tool::ToolRegistry;

/// What the turn driver should do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnStep {
    CallModel {
        allow_tools: bool,
    },
    /// Execute the complete, ordered tool batch stored in [`TurnContext`].
    CallTool,
    Done {
        finish: FinishReason,
        last_agent_message: Option<String>,
    },
}

/// Result produced by a successful model or complete tool-batch step.
#[derive(Debug, Clone, PartialEq)]
pub enum StepOutput<'a> {
    Model(&'a ModelResponse),
    Tools(&'a [ToolResult]),
}

/// Successful outcome of one turn, before completion events are recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnOutcome {
    pub finish: FinishReason,
    pub last_agent_message: Option<String>,
}

/// Model, tools and middleware bound to an [`Agent`].
#[derive(Clone)]
pub struct AgentBindings {
    model: Arc<dyn ModelClient>,
    tools: Arc<ToolRegistry>,
    middlewares: Vec<Arc<dyn Middleware>>,
}

impl AgentBindings {
    pub fn new(model: Arc<dyn ModelClient>, tools: Arc<ToolRegistry>) -> Self {
        Self {
            model,
            tools,
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

    pub fn push_middleware(&mut self, middleware: Arc<dyn Middleware>) {
        self.middlewares.push(middleware);
    }

    pub fn model(&self) -> &Arc<dyn ModelClient> {
        &self.model
    }

    pub fn tools(&self) -> &Arc<ToolRegistry> {
        &self.tools
    }

    pub fn middlewares(&self) -> &[Arc<dyn Middleware>] {
        &self.middlewares
    }
}

/// Mutable state belonging to one running turn.
pub struct TurnContext<'a> {
    pub recorder: &'a mut Recorder,
    pub config: &'a ThreadConfig,
    pub turn_id: TurnId,
    iteration: u32,
    model_calls: u64,
    usage: TokenUsage,
    open_iteration: Option<u32>,
    open_usage: Option<TokenUsage>,
    initial_step: Option<TurnStep>,
    tool_batch: Option<Vec<ToolCall>>,
    reminders: Arc<ReminderQueue>,
}

impl<'a> TurnContext<'a> {
    pub fn new(recorder: &'a mut Recorder, config: &'a ThreadConfig, turn_id: TurnId) -> Self {
        Self {
            recorder,
            config,
            turn_id,
            iteration: 0,
            model_calls: 0,
            usage: TokenUsage::default(),
            open_iteration: None,
            open_usage: None,
            initial_step: None,
            tool_batch: None,
            reminders: Arc::new(ReminderQueue::default()),
        }
    }

    /// Zero-based index of the next model call.
    pub fn iteration(&self) -> u32 {
        self.iteration
    }

    /// Number of model calls started in this turn.
    pub fn model_calls(&self) -> u64 {
        self.model_calls
    }

    pub fn open_model_iteration(&self) -> Option<u32> {
        self.open_iteration
    }

    /// Token usage summed over the model calls completed so far.
    pub fn usage(&self) -> TokenUsage {
        self.usage
    }

    pub fn hook_ctx(&self, iteration: Option<u32>) -> HookCtx<'a> {
        HookCtx::with_queue(
            self.recorder.thread_id(),
            self.turn_id,
            iteration,
            self.config,
            Arc::clone(&self.reminders),
        )
    }

    /// Queue a reminder; it is recorded by the next [`TurnContext::flush_reminders`].
    pub fn remind(&self, reminder: SystemReminder) {
        self.reminders.push(reminder);
    }

    /// Record every queued reminder, in the order added, and return the events.
    ///
    /// A recorded reminder is consumed by the first `ModelRequestCompleted`
    /// after it, so drivers flush only where the next request should see it.
    pub async fn flush_reminders(&mut self) -> Result<Vec<Event>, CoreError> {
        let mut events = Vec::new();
        for reminder in self.reminders.drain() {
            events.push(
                self.recorder
                    .record(EventMsg::SystemReminder(SystemReminderEvent { reminder }))
                    .await?,
            );
        }
        Ok(events)
    }

    /// Set the plan selected by the turn-level `before_turn` hooks.
    pub fn set_initial_step(&mut self, step: TurnStep) {
        self.initial_step = Some(step);
    }

    /// Consume the turn-level plan. It cannot be consumed twice.
    pub fn take_initial_step(&mut self) -> Option<TurnStep> {
        self.initial_step.take()
    }

    /// Whether an ordered tool batch is waiting to be executed.
    pub fn has_tool_batch(&self) -> bool {
        self.tool_batch.is_some()
    }

    /// Store one not-yet-executed batch. A prior batch must first be consumed.
    pub fn set_tool_batch(&mut self, calls: Vec<ToolCall>) -> Result<(), CoreError> {
        if calls.is_empty() {
            return Err(CoreError::Internal(
                "cannot store an empty tool batch".into(),
            ));
        }
        if self.tool_batch.is_some() {
            return Err(CoreError::Internal(
                "a tool batch is already pending".into(),
            ));
        }
        self.tool_batch = Some(calls);
        Ok(())
    }

    /// Consume the pending batch exactly once.
    pub fn take_tool_batch(&mut self) -> Result<Vec<ToolCall>, CoreError> {
        self.tool_batch
            .take()
            .ok_or_else(|| CoreError::Internal("no pending tool batch".into()))
    }

    /// Begin the model iteration identified by [`TurnContext::iteration`].
    pub fn begin_model_iteration(&mut self) -> Result<u32, CoreError> {
        let max_calls = u64::from(self.config.max_tool_iterations) + 1;
        if self.model_calls >= max_calls {
            return Err(CoreError::Internal(format!(
                "agent requested more than {max_calls} model calls"
            )));
        }
        if self.open_iteration.is_some() {
            return Err(CoreError::Internal(
                "a model iteration is already open".into(),
            ));
        }
        self.open_iteration = Some(self.iteration);
        self.open_usage = None;
        Ok(self.iteration)
    }

    /// Set usage exactly once for the current model iteration.
    pub fn set_model_usage(&mut self, usage: TokenUsage) -> Result<(), CoreError> {
        if self.open_iteration.is_none() || self.open_usage.is_some() {
            return Err(CoreError::Internal(
                "model usage has no unique open iteration".into(),
            ));
        }
        self.open_usage = Some(usage);
        Ok(())
    }

    /// Record the current iteration's completion, add its usage once and
    /// advance the model-call counter.
    pub async fn finish_model_iteration(&mut self) -> Result<(), CoreError> {
        let iteration = self
            .open_iteration
            .take()
            .ok_or_else(|| CoreError::Internal("no model iteration is open".into()))?;
        let usage = self
            .open_usage
            .take()
            .ok_or_else(|| CoreError::Internal("open model iteration has no usage".into()))?;
        self.recorder
            .record(EventMsg::ModelRequestCompleted(
                ModelRequestCompletedEvent { iteration, usage },
            ))
            .await?;
        self.usage += usage;
        self.model_calls += 1;
        if let Some(next) = iteration.checked_add(1) {
            self.iteration = next;
        }
        Ok(())
    }
}

/// Text streamed as deltas for one assistant message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamedMessage {
    pub message_id: MessageId,
    pub text: String,
}

/// Result of one reasoning step.
#[derive(Debug, Clone, PartialEq)]
pub struct ReasoningOutput {
    pub response: ModelResponse,
    /// Present when at least one non-empty delta was streamed and recorded.
    pub streamed: Option<StreamedMessage>,
}

/// A per-turn ReAct agent.
///
/// The default [`Agent::forward`] is the only built-in scheduler. It calls
/// the overridable reasoning and tool methods through their middleware
/// wrappers; custom turn flows can override `forward` and still use
/// `forward_with_hooks` for turn hooks and completion events.
///
/// All middleware hooks run in registration order.
#[async_trait]
pub trait Agent: Send + Sync {
    fn bindings(&self) -> &AgentBindings;

    fn bindings_mut(&mut self) -> &mut AgentBindings;

    fn model(&self) -> &Arc<dyn ModelClient> {
        self.bindings().model()
    }

    fn tools(&self) -> &ToolRegistry {
        self.bindings().tools()
    }

    fn middlewares(&self) -> &[Arc<dyn Middleware>] {
        self.bindings().middlewares()
    }

    fn bind_middleware(&mut self, middleware: Arc<dyn Middleware>) {
        self.bindings_mut().push_middleware(middleware);
    }

    /// Consumes one model stream and records its text deltas.
    async fn reasoning(
        &self,
        ctx: &mut TurnContext<'_>,
        request: ModelRequest,
    ) -> Result<ReasoningOutput, CoreError> {
        let mut stream = self.model().stream(request).await?;
        let mut streamed: Option<StreamedMessage> = None;
        while let Some(item) = stream.next().await {
            match item? {
                ModelStreamItem::TextDelta { text } if text.is_empty() => {}
                ModelStreamItem::TextDelta { text } => {
                    let message_id = match &mut streamed {
                        Some(message) => {
                            message.text.push_str(&text);
                            message.message_id
                        }
                        None => {
                            let message_id = MessageId::new();
                            ctx.recorder
                                .record(EventMsg::AssistantMessageStart(
                                    AssistantMessageStartEvent { message_id },
                                ))
                                .await?;
                            streamed = Some(StreamedMessage {
                                message_id,
                                text: text.clone(),
                            });
                            message_id
                        }
                    };
                    ctx.recorder
                        .record(EventMsg::AssistantMessageDelta(
                            AssistantMessageDeltaEvent {
                                message_id,
                                delta: text,
                            },
                        ))
                        .await?;
                }
                ModelStreamItem::Completed { response } => {
                    return Ok(ReasoningOutput { response, streamed });
                }
            }
        }
        Err(CoreError::Model(
            "model stream ended without a final response".into(),
        ))
    }

    /// Records the request start, assembles the event snapshot, runs the
    /// read-only model hooks, then calls the overridable [`Agent::reasoning`].
    ///
    /// Reminders written before or by `before_model` are recorded before the
    /// snapshot is final, so this request delivers them. Reminders written by
    /// `after_model` stay queued for the driver to flush.
    async fn reasoning_with_hooks(
        &self,
        ctx: &mut TurnContext<'_>,
        allow_tools: bool,
    ) -> Result<ReasoningOutput, CoreError> {
        let iteration = ctx.iteration();
        ctx.recorder
            .record(EventMsg::ModelRequestStarted(ModelRequestStartedEvent {
                iteration,
                allow_tools,
            }))
            .await?;
        ctx.flush_reminders().await?;
        let mut events = ctx.recorder.events().await?;
        let hook_ctx = ctx.hook_ctx(Some(iteration));
        for middleware in self.middlewares() {
            middleware.before_model(&hook_ctx, &events).await?;
        }
        events.extend(ctx.flush_reminders().await?);
        let request = ModelRequest {
            system_prompt: ctx.config.effective_system_prompt(),
            events,
            tools: if allow_tools {
                self.tools().specs()
            } else {
                Vec::new()
            },
            iteration,
        };
        let output = self.reasoning(ctx, request).await?;
        for middleware in self.middlewares() {
            middleware.after_model(&hook_ctx, &output.response).await?;
        }
        Ok(output)
    }

    /// Executes one tool call. This is the overridable, single-call execution
    /// point; event recording and middleware wrapping belong to
    /// [`Agent::acting_with_hooks`].
    async fn acting(&self, call: &ToolCall) -> ToolResult {
        match self.tools().get(&call.name) {
            Some(tool) => tool.execute(call).await,
            None => ToolResult::not_found(call),
        }
    }

    /// Executes a complete ordered batch, wrapping every call individually.
    ///
    /// A `ToolDecision::Skip` stops the remaining `before_tool` hooks for that
    /// call and substitutes its result; all `after_tool` hooks still run.
    async fn acting_with_hooks(
        &self,
        ctx: &mut TurnContext<'_>,
        calls: Vec<ToolCall>,
    ) -> Result<Vec<ToolResult>, CoreError> {
        let mut results = Vec::with_capacity(calls.len());
        for call in calls {
            results.push(run_tool_call(self, ctx, call).await?);
        }
        Ok(results)
    }

    /// Runs the standard three-step scheduler and returns its outcome.
    ///
    /// This method does not run turn hooks or record completion events, but
    /// it does run model, tool, and step hooks.
    async fn forward(&mut self, ctx: &mut TurnContext<'_>) -> Result<TurnOutcome, CoreError> {
        let mut step = match ctx.take_initial_step() {
            Some(step) => step,
            None => {
                let step = initial_step(ctx);
                ctx.flush_reminders().await?;
                step
            }
        };

        loop {
            match step {
                TurnStep::Done {
                    finish,
                    last_agent_message,
                } => {
                    return Ok(TurnOutcome {
                        finish,
                        last_agent_message,
                    });
                }
                TurnStep::CallTool => {
                    let calls = ctx.take_tool_batch()?;
                    let results = self.acting_with_hooks(ctx, calls).await?;
                    let next_iteration = ctx.iteration().checked_add(1);
                    let next = match next_iteration {
                        Some(iteration) if iteration < ctx.config.max_tool_iterations => {
                            TurnStep::CallModel { allow_tools: true }
                        }
                        Some(_) => {
                            ctx.remind(SystemReminder::ToolLimitReached);
                            TurnStep::CallModel { allow_tools: false }
                        }
                        None => TurnStep::Done {
                            finish: FinishReason::ExceedMaxIters,
                            last_agent_message: None,
                        },
                    };
                    let mut next = next;
                    self.schedule_after_step(ctx, StepOutput::Tools(&results), &mut next)
                        .await?;
                    ctx.finish_model_iteration().await?;
                    ctx.flush_reminders().await?;
                    step = next;
                }
                TurnStep::CallModel { allow_tools } => {
                    if allow_tools && ctx.iteration() >= ctx.config.max_tool_iterations {
                        return Err(CoreError::Internal(
                            "agent enabled tools after the tool iteration budget".into(),
                        ));
                    }
                    let iteration = ctx.begin_model_iteration()?;
                    let ReasoningOutput { response, streamed } =
                        self.reasoning_with_hooks(ctx, allow_tools).await?;
                    ctx.set_model_usage(response.usage)?;
                    let content = response
                        .content
                        .clone()
                        .or_else(|| streamed.as_ref().map(|message| message.text.clone()));
                    let calls = if allow_tools {
                        response.tool_calls.clone()
                    } else {
                        Vec::new()
                    };
                    let ignored = response.tool_calls.len().saturating_sub(calls.len());
                    if !calls.is_empty() {
                        ctx.set_tool_batch(calls.clone())?;
                    }

                    let default_next = if !calls.is_empty() {
                        TurnStep::CallTool
                    } else {
                        TurnStep::Done {
                            finish: if iteration >= ctx.config.max_tool_iterations {
                                FinishReason::ExceedMaxIters
                            } else {
                                FinishReason::Completed
                            },
                            last_agent_message: content.clone(),
                        }
                    };
                    let mut next = default_next;
                    self.schedule_after_step(ctx, StepOutput::Model(&response), &mut next)
                        .await?;

                    let execute_batch = matches!(next, TurnStep::CallTool);
                    let retained_calls = if execute_batch {
                        calls.clone()
                    } else {
                        if ctx.has_tool_batch() {
                            let _ = ctx.take_tool_batch()?;
                        }
                        Vec::new()
                    };
                    let ignored = ignored + if execute_batch { 0 } else { calls.len() };
                    record_assistant_message(ctx, streamed, content, retained_calls).await?;
                    if ignored > 0 {
                        ctx.recorder
                            .record(EventMsg::Warning(WarningEvent {
                                message: format!("ignored {ignored} tool call(s)"),
                            }))
                            .await?;
                    }
                    if !execute_batch {
                        ctx.finish_model_iteration().await?;
                        ctx.flush_reminders().await?;
                    }
                    step = next;
                }
            }
        }
    }

    /// Runs turn hooks around the overridable flow and records completion once.
    async fn forward_with_hooks(&mut self, ctx: &mut TurnContext<'_>) -> Result<(), CoreError> {
        let hook_ctx = ctx.hook_ctx(None);
        let mut initial = initial_step(ctx);
        for middleware in self.middlewares() {
            middleware.before_turn(&hook_ctx, &mut initial).await?;
        }
        ctx.set_initial_step(initial);
        ctx.flush_reminders().await?;
        let outcome = self.forward(ctx).await?;
        for middleware in self.middlewares() {
            middleware.after_turn(&hook_ctx).await?;
        }
        complete_turn(ctx, outcome).await
    }

    async fn schedule_after_step(
        &self,
        ctx: &TurnContext<'_>,
        output: StepOutput<'_>,
        next: &mut TurnStep,
    ) -> Result<(), CoreError> {
        let hook_ctx = ctx.hook_ctx(Some(ctx.iteration()));
        let mut locked_done = match next {
            TurnStep::Done { .. } => Some(next.clone()),
            _ => None,
        };
        for middleware in self.middlewares() {
            middleware.after_step(&hook_ctx, &output, next).await?;
            if let Some(locked) = &locked_done {
                if next != locked {
                    return Err(CoreError::Internal(
                        "middleware attempted to change a locked Done step".into(),
                    ));
                }
            } else if matches!(next, TurnStep::Done { .. }) {
                locked_done = Some(next.clone());
            }
        }
        match next {
            TurnStep::CallTool if !ctx.has_tool_batch() => Err(CoreError::Internal(
                "agent selected CallTool without a pending tool batch".into(),
            )),
            TurnStep::CallModel { allow_tools }
                if *allow_tools
                    && ctx
                        .open_iteration
                        .and_then(|iteration| iteration.checked_add(1))
                        .unwrap_or(ctx.iteration())
                        >= ctx.config.max_tool_iterations =>
            {
                Err(CoreError::Internal(
                    "agent enabled tools after the tool iteration budget".into(),
                ))
            }
            _ => Ok(()),
        }
    }
}

/// Creates a fresh per-turn agent from its bindings and the thread configuration.
pub type AgentFactory = Arc<dyn Fn(AgentBindings, &ThreadConfig) -> Box<dyn Agent> + Send + Sync>;

/// First step of a turn; queues the limit reminder when no tool call is allowed.
fn initial_step(ctx: &TurnContext<'_>) -> TurnStep {
    let allow_tools = ctx.iteration() < ctx.config.max_tool_iterations;
    if !allow_tools {
        ctx.remind(SystemReminder::ToolLimitReached);
    }
    TurnStep::CallModel { allow_tools }
}

async fn record_assistant_message(
    ctx: &mut TurnContext<'_>,
    streamed: Option<StreamedMessage>,
    content: Option<String>,
    tool_calls: Vec<ToolCall>,
) -> Result<(), CoreError> {
    if content.is_none() && tool_calls.is_empty() {
        return Ok(());
    }
    let message_id = streamed
        .as_ref()
        .map(|message| message.message_id)
        .unwrap_or_default();
    if streamed.is_none() {
        ctx.recorder
            .record(EventMsg::AssistantMessageStart(
                AssistantMessageStartEvent { message_id },
            ))
            .await?;
    }
    ctx.recorder
        .record(EventMsg::AssistantMessage(AssistantMessageEvent {
            message_id,
            content,
            tool_calls,
        }))
        .await?;
    Ok(())
}

async fn run_tool_call<A: Agent + ?Sized>(
    agent: &A,
    ctx: &mut TurnContext<'_>,
    mut call: ToolCall,
) -> Result<ToolResult, CoreError> {
    let hook_ctx = ctx.hook_ctx(Some(ctx.iteration()));
    let original_id = call.id.clone();
    let original_name = call.name.clone();
    let mut decision = ToolDecision::Continue;
    for middleware in agent.middlewares() {
        decision = middleware.before_tool(&hook_ctx, &mut call).await?;
        if matches!(decision, ToolDecision::Skip(_)) {
            break;
        }
    }
    call.id = original_id;
    call.name = original_name;
    ctx.recorder
        .record(EventMsg::ToolCallBegin(ToolCallBeginEvent {
            tool_call_id: call.id.clone(),
            tool_name: call.name.clone(),
            arguments: call.arguments.clone(),
        }))
        .await?;

    let started = Instant::now();
    let skipped = matches!(decision, ToolDecision::Skip(_));
    let mut result = match decision {
        ToolDecision::Skip(result) => result,
        ToolDecision::Continue => agent.acting(&call).await,
    };
    for middleware in agent.middlewares() {
        middleware.after_tool(&hook_ctx, &call, &mut result).await?;
    }
    result.call_id = call.id.clone();
    result.tool_name = call.name.clone();
    let duration_ms = if skipped {
        0
    } else {
        started.elapsed().as_millis() as u64
    };
    ctx.recorder
        .record(EventMsg::ToolCallEnd(ToolCallEndEvent {
            result: result.clone(),
            duration_ms: Some(duration_ms),
        }))
        .await?;
    Ok(result)
}

async fn complete_turn(ctx: &mut TurnContext<'_>, outcome: TurnOutcome) -> Result<(), CoreError> {
    ctx.flush_reminders().await?;
    ctx.recorder
        .record(EventMsg::TokenCount(TokenCountEvent { usage: ctx.usage }))
        .await?;
    ctx.recorder
        .record(EventMsg::TurnComplete(TurnCompleteEvent {
            last_agent_message: outcome.last_agent_message,
            finish_reason: outcome.finish,
        }))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tapestry_protocol::{ThreadId, ToolCallId};

    use super::*;
    use crate::clock::FixedClock;
    use crate::model::MockModelClient;
    use crate::store::InMemoryEventStore;
    use crate::tool::ToolRegistry;

    #[test]
    fn context_consumes_initial_plan_and_batches_once() {
        let store = Arc::new(InMemoryEventStore::new());
        let clock = Arc::new(FixedClock::new("2026-11-23", "UTC"));
        let mut recorder = Recorder::new(store, clock, ThreadId::new());
        let config = ThreadConfig::default();
        let mut ctx = TurnContext::new(&mut recorder, &config, TurnId::new());

        ctx.set_initial_step(TurnStep::Done {
            finish: FinishReason::Completed,
            last_agent_message: None,
        });
        assert!(matches!(
            ctx.take_initial_step(),
            Some(TurnStep::Done { .. })
        ));
        assert!(ctx.take_initial_step().is_none());

        let calls = vec![ToolCall::new(
            ToolCallId::from("a"),
            "echo",
            serde_json::Value::Null,
        )];
        ctx.set_tool_batch(calls.clone()).unwrap();
        assert!(ctx.set_tool_batch(Vec::new()).is_err());
        assert_eq!(ctx.take_tool_batch().unwrap(), calls);
        assert!(ctx.take_tool_batch().is_err());

        let other = TurnContext::new(&mut recorder, &config, TurnId::new());
        assert_eq!(other.iteration(), 0);
        assert_eq!(other.usage(), TokenUsage::default());
        assert!(!other.has_tool_batch());
    }

    #[tokio::test]
    async fn flush_records_queued_reminders_in_order_through_hook_contexts() {
        let store = Arc::new(InMemoryEventStore::new());
        let clock = Arc::new(FixedClock::new("2026-11-23", "UTC"));
        let mut recorder = Recorder::new(store, clock, ThreadId::new());
        let config = ThreadConfig::default();
        let mut ctx = TurnContext::new(&mut recorder, &config, TurnId::new());

        ctx.remind(SystemReminder::custom("agent"));
        ctx.hook_ctx(None).remind(SystemReminder::custom("hook"));
        let events = ctx.flush_reminders().await.unwrap();

        let texts: Vec<_> = events
            .iter()
            .map(|event| match &event.msg {
                EventMsg::SystemReminder(r) => r.reminder.clone(),
                other => panic!("unexpected event {other:?}"),
            })
            .collect();
        assert_eq!(
            texts,
            [
                SystemReminder::custom("agent"),
                SystemReminder::custom("hook")
            ]
        );
        assert!(ctx.flush_reminders().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn flush_propagates_event_store_failure() {
        let store = Arc::new(crate::store::testing::FlakyStore::default());
        store.fail_after(0);
        let clock = Arc::new(FixedClock::new("2026-11-23", "UTC"));
        let mut recorder = Recorder::new(store, clock, ThreadId::new());
        let config = ThreadConfig::default();
        let mut ctx = TurnContext::new(&mut recorder, &config, TurnId::new());

        ctx.remind(SystemReminder::ToolLimitReached);
        assert!(matches!(
            ctx.flush_reminders().await,
            Err(CoreError::EventStore(_))
        ));
    }

    #[tokio::test]
    async fn model_iteration_counter_overflow_is_checked_without_wrapping() {
        let store = Arc::new(InMemoryEventStore::new());
        let clock = Arc::new(FixedClock::new("2026-11-23", "UTC"));
        let mut recorder = Recorder::new(store, clock, ThreadId::new());
        let config = ThreadConfig {
            max_tool_iterations: u32::MAX,
            ..ThreadConfig::default()
        };
        let mut ctx = TurnContext::new(&mut recorder, &config, TurnId::new());
        ctx.iteration = u32::MAX;
        ctx.model_calls = u64::from(u32::MAX);

        assert_eq!(ctx.begin_model_iteration().unwrap(), u32::MAX);
        ctx.set_model_usage(TokenUsage::default()).unwrap();
        ctx.finish_model_iteration().await.unwrap();

        assert_eq!(ctx.model_calls(), u64::from(u32::MAX) + 1);
        assert_eq!(ctx.iteration(), u32::MAX);
        assert!(ctx.begin_model_iteration().is_err());
    }

    #[tokio::test]
    async fn default_forward_uses_standard_react_schedule() {
        let store = Arc::new(InMemoryEventStore::new());
        let clock = Arc::new(FixedClock::new("2026-11-23", "UTC"));
        let mut recorder = Recorder::new(store, clock, ThreadId::new());
        let config = ThreadConfig {
            include_time_context: false,
            ..ThreadConfig::default()
        };
        let bindings = AgentBindings::new(Arc::new(MockModelClient), Arc::new(ToolRegistry::new()));
        let mut agent = crate::agent::ReactAgent::new(bindings, 1);
        let mut ctx = TurnContext::new(&mut recorder, &config, TurnId::new());
        agent.forward_with_hooks(&mut ctx).await.unwrap();
    }
}
