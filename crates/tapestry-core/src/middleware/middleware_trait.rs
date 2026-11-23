use std::sync::Arc;

use async_trait::async_trait;
use tapestry_protocol::{
    Event, ModelResponse, SystemReminder, ThreadId, ToolCall, ToolResult, TurnId,
};

use crate::agent::{StepOutput, TurnStep};
use crate::config::ThreadConfig;
use crate::error::CoreError;
use crate::reminder::ReminderQueue;

/// Context shared by middleware hooks. It is read-only except for
/// [`HookCtx::remind`], the only way a hook can add to the event log.
pub struct HookCtx<'a> {
    pub thread_id: ThreadId,
    pub turn_id: TurnId,
    /// Present for model and tool hooks; absent for turn hooks.
    pub iteration: Option<u32>,
    pub config: &'a ThreadConfig,
    reminders: Arc<ReminderQueue>,
}

impl<'a> HookCtx<'a> {
    /// Create a context with its own reminder queue.
    pub fn new(
        thread_id: ThreadId,
        turn_id: TurnId,
        iteration: Option<u32>,
        config: &'a ThreadConfig,
    ) -> Self {
        Self::with_queue(
            thread_id,
            turn_id,
            iteration,
            config,
            Arc::new(ReminderQueue::default()),
        )
    }

    pub(crate) fn with_queue(
        thread_id: ThreadId,
        turn_id: TurnId,
        iteration: Option<u32>,
        config: &'a ThreadConfig,
        reminders: Arc<ReminderQueue>,
    ) -> Self {
        Self {
            thread_id,
            turn_id,
            iteration,
            config,
            reminders,
        }
    }

    /// Queue a system reminder. Core records it as a `SystemReminder` event at
    /// the next safe point; the next model request delivers it.
    pub fn remind(&self, reminder: SystemReminder) {
        self.reminders.push(reminder);
    }
}

/// Decision returned by a `before_tool` hook.
///
/// `Skip` uses the supplied result without executing the tool; the core still
/// records the matching begin/end events and runs all `after_tool` hooks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolDecision {
    Continue,
    Skip(ToolResult),
}

/// Async extension hooks around agent turns, model calls, and tool calls.
///
/// Every hook runs in registration order. Model hooks only observe: they influence the model context solely through
/// [`HookCtx::remind`]. Middleware state belongs to the middleware instance.
#[async_trait]
pub trait Middleware: Send + Sync {
    async fn before_turn(
        &self,
        _ctx: &HookCtx<'_>,
        _initial_step: &mut TurnStep,
    ) -> Result<(), CoreError> {
        Ok(())
    }

    async fn after_turn(&self, _ctx: &HookCtx<'_>) -> Result<(), CoreError> {
        Ok(())
    }

    /// Observe the event snapshot of this request. Reminders written here are
    /// appended to the snapshot and delivered by this request.
    async fn before_model(&self, _ctx: &HookCtx<'_>, _events: &[Event]) -> Result<(), CoreError> {
        Ok(())
    }

    /// Observe the final response. Reminders written here reach the next request.
    async fn after_model(
        &self,
        _ctx: &HookCtx<'_>,
        _response: &ModelResponse,
    ) -> Result<(), CoreError> {
        Ok(())
    }

    async fn before_tool(
        &self,
        _ctx: &HookCtx<'_>,
        _call: &mut ToolCall,
    ) -> Result<ToolDecision, CoreError> {
        Ok(ToolDecision::Continue)
    }

    async fn after_tool(
        &self,
        _ctx: &HookCtx<'_>,
        _call: &ToolCall,
        _result: &mut ToolResult,
    ) -> Result<(), CoreError> {
        Ok(())
    }

    async fn after_step(
        &self,
        _ctx: &HookCtx<'_>,
        _output: &StepOutput<'_>,
        _next_step: &mut TurnStep,
    ) -> Result<(), CoreError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tapestry_protocol::{ModelResponse, ThreadId, TokenUsage, ToolCall, ToolCallId, TurnId};

    use super::*;

    struct Reminding;

    #[async_trait]
    impl Middleware for Reminding {
        async fn before_model(
            &self,
            ctx: &HookCtx<'_>,
            _events: &[Event],
        ) -> Result<(), CoreError> {
            ctx.remind(SystemReminder::custom("a"));
            ctx.remind(SystemReminder::custom("b"));
            Ok(())
        }
    }

    struct Noop;

    #[async_trait]
    impl Middleware for Noop {}

    struct StepHook;

    #[async_trait]
    impl Middleware for StepHook {
        async fn after_step(
            &self,
            _ctx: &HookCtx<'_>,
            _output: &StepOutput<'_>,
            next_step: &mut TurnStep,
        ) -> Result<(), CoreError> {
            *next_step = TurnStep::Done {
                finish: tapestry_protocol::FinishReason::Completed,
                last_agent_message: Some("done".into()),
            };
            Ok(())
        }
    }

    #[tokio::test]
    async fn unimplemented_hooks_are_noops() {
        let config = ThreadConfig::default();
        let ctx = HookCtx::new(ThreadId::new(), TurnId::new(), Some(0), &config);
        let response = ModelResponse {
            content: Some("hello".into()),
            tool_calls: Vec::new(),
            usage: TokenUsage::default(),
        };
        let mut call = ToolCall::new(ToolCallId::generate(), "echo", json!({"input":"x"}));
        let original_call = call.clone();
        let mut result = ToolResult::completed(&call, "x");
        let original_result = result.clone();
        let mut step = TurnStep::CallModel { allow_tools: true };

        Noop.before_turn(&ctx, &mut step).await.unwrap();
        Noop.before_model(&ctx, &[]).await.unwrap();
        Noop.after_model(&ctx, &response).await.unwrap();
        assert_eq!(
            Noop.before_tool(&ctx, &mut call).await.unwrap(),
            ToolDecision::Continue
        );
        Noop.after_tool(&ctx, &call, &mut result).await.unwrap();
        Noop.after_turn(&ctx).await.unwrap();

        assert_eq!(step, TurnStep::CallModel { allow_tools: true });
        assert_eq!(call, original_call);
        assert_eq!(result, original_result);
        assert!(ctx.reminders.drain().is_empty());
    }

    #[tokio::test]
    async fn hooks_queue_reminders_in_order() {
        let config = ThreadConfig::default();
        let ctx = HookCtx::new(ThreadId::new(), TurnId::new(), Some(0), &config);
        Reminding.before_model(&ctx, &[]).await.unwrap();
        assert_eq!(
            ctx.reminders.drain(),
            vec![SystemReminder::custom("a"), SystemReminder::custom("b")]
        );
        assert!(ctx.reminders.drain().is_empty());
    }

    #[tokio::test]
    async fn only_after_step_implementation_is_sufficient() {
        let config = ThreadConfig::default();
        let ctx = HookCtx::new(ThreadId::new(), TurnId::new(), Some(0), &config);
        let middleware = StepHook;
        let mut next = TurnStep::CallModel { allow_tools: true };
        middleware
            .after_step(&ctx, &StepOutput::Tools(&[]), &mut next)
            .await
            .unwrap();
        assert_eq!(
            next,
            TurnStep::Done {
                finish: tapestry_protocol::FinishReason::Completed,
                last_agent_message: Some("done".into()),
            }
        );
        middleware.after_turn(&ctx).await.unwrap();
    }
}
