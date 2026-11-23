use async_trait::async_trait;
use tapestry_protocol::{ModelRequest, ModelResponse, ThreadId, ToolCall, ToolResult, TurnId};

use crate::agent::{StepOutput, TurnStep};
use crate::config::ThreadConfig;
use crate::error::CoreError;

/// Read-only context shared by middleware hooks.
pub struct HookCtx<'a> {
    pub thread_id: ThreadId,
    pub turn_id: TurnId,
    /// Present for model and tool hooks; absent for turn hooks.
    pub iteration: Option<u32>,
    pub config: &'a ThreadConfig,
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
/// Every hook runs in registration order. Middleware state belongs to the
/// middleware instance.
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

    async fn before_model(
        &self,
        _ctx: &HookCtx<'_>,
        _request: &mut ModelRequest,
    ) -> Result<(), CoreError> {
        Ok(())
    }

    /// If adding tool calls, assign each call an id; use
    /// `ToolCallId::generate()` when there is no provider-supplied id.
    async fn after_model(
        &self,
        _ctx: &HookCtx<'_>,
        _response: &mut ModelResponse,
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
    use tapestry_protocol::{
        ModelRequest, ModelResponse, ThreadId, TokenUsage, ToolCall, ToolCallId, TurnId,
    };

    use super::*;

    struct OneHook;

    #[async_trait]
    impl Middleware for OneHook {
        async fn before_model(
            &self,
            _ctx: &HookCtx<'_>,
            request: &mut ModelRequest,
        ) -> Result<(), CoreError> {
            request.system_prompt = Some("patched".into());
            Ok(())
        }
    }

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
        let ctx = HookCtx {
            thread_id: ThreadId::new(),
            turn_id: TurnId::new(),
            iteration: Some(0),
            config: &config,
        };
        let middleware = OneHook;
        let mut request = ModelRequest {
            system_prompt: None,
            events: Vec::new(),
            tools: Vec::new(),
            iteration: 0,
        };
        let mut response = ModelResponse {
            content: Some("hello".into()),
            tool_calls: Vec::new(),
            usage: TokenUsage::default(),
        };
        let mut call = ToolCall::new(ToolCallId::generate(), "echo", json!({"input":"x"}));
        let original_call = call.clone();
        let mut result = ToolResult::completed(&call, "x");
        let original_result = result.clone();

        middleware
            .before_turn(
                &ctx,
                &mut TurnStep::CallModel {
                    allow_tools: true,
                    reminder: None,
                },
            )
            .await
            .unwrap();
        middleware.after_turn(&ctx).await.unwrap();
        assert_eq!(
            middleware.before_tool(&ctx, &mut call).await.unwrap(),
            ToolDecision::Continue
        );
        middleware
            .after_tool(&ctx, &call, &mut result)
            .await
            .unwrap();
        middleware.after_model(&ctx, &mut response).await.unwrap();

        assert_eq!(call, original_call);
        assert_eq!(result, original_result);
        assert_eq!(
            response,
            ModelResponse {
                content: Some("hello".into()),
                tool_calls: Vec::new(),
                usage: TokenUsage::default(),
            }
        );
        assert_eq!(request.system_prompt, None);
        middleware.before_model(&ctx, &mut request).await.unwrap();
        assert_eq!(request.system_prompt.as_deref(), Some("patched"));
        assert!(request.events.is_empty());
        assert!(request.tools.is_empty());
    }

    #[tokio::test]
    async fn only_after_step_implementation_is_sufficient() {
        let config = ThreadConfig::default();
        let ctx = HookCtx {
            thread_id: ThreadId::new(),
            turn_id: TurnId::new(),
            iteration: Some(0),
            config: &config,
        };
        let middleware = StepHook;
        let mut next = TurnStep::CallModel {
            allow_tools: true,
            reminder: None,
        };
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
