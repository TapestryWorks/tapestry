//! Tapestry agent core: thread lifecycle, agent loop, and model interaction.
//!
//! Architecture is informed by OpenAI Codex `codex-core`; see `docs/architecture.md`.

pub mod agent;
pub mod builtin_tools;
pub mod clock;
pub mod config;
pub mod context;
pub mod error;
pub mod formatter;
pub mod middleware;
pub mod model;
pub mod projection;
pub mod recorder;
mod reminder;
pub mod store;
pub mod thread;
pub mod tool;

pub use agent::AgentLoop;
pub use agent::{
    Agent, AgentDeps, AgentFactory, ReactAgent, ReasoningOutput, StepOutput, StreamedMessage,
    TurnContext, TurnOutcome, TurnStep,
};
pub use builtin_tools::{BuiltinToolsConfig, ReadFileTool, ShellTool, WriteFileTool};
pub use clock::{Clock, ClockError, ClockReading, FixedClock, SystemClock};
pub use config::ThreadConfig;
pub use error::CoreError;
pub use formatter::prompt::{
    render_environment_context, render_system_reminder, render_user, FORCED_FINISH_REMINDER,
};
pub use formatter::Formatter;
pub use formatter::OpenAiFormatter;
pub use middleware::{HookCtx, Middleware, ToolDecision};
pub use model::{
    ChatModel, MockChatModel, MockModelClient, MockResponseModel, ModelClient, ModelStream,
    ModelStreamSource, ResponseModel,
};
pub use projection::ConversationProjection;
pub use tapestry_protocol::{
    ConversationItem, EnvironmentContext, ModelRequest, ModelResponse, SystemReminder, TokenUsage,
    ToolCallId, ToolResult, ToolSpec,
};

pub use model::genai::{
    default_api_key_env, parse_model_name, GenaiChatService, GenaiModelClient, GenaiProviderConfig,
};
pub use model::{OpenAiChatModel, OpenAiResponseModel};
pub use recorder::Recorder;
pub use store::{EventStore, EventSubscription, InMemoryEventStore, NewEvent, SubscriptionSource};
pub use thread::{TapestryThread, ThreadBuilder, ThreadHandle};
pub use tool::{EchoTool, Tool, ToolRegistry};

#[cfg(test)]
mod compatibility_tests {
    #[test]
    fn grouped_model_paths_match_crate_root_exports() {
        use crate::model::{chat_model, response_model};

        let chat: &dyn chat_model::ChatModel = &crate::MockChatModel;
        let _: &dyn crate::ChatModel = chat;
        let response: &dyn response_model::ResponseModel = &crate::MockResponseModel;
        let _: &dyn crate::ResponseModel = response;
    }

    #[test]
    fn legacy_model_and_formatter_paths_remain_available() {
        fn assert_model_client<T: crate::model::ModelClient>() {}
        assert_model_client::<crate::model::MockModelClient>();

        let _: crate::projection::ConversationProjection =
            crate::formatter::formatter_trait::ConversationProjection::new();
        assert_eq!(
            crate::formatter::prompt::render_user(None, "hello"),
            "hello"
        );
    }

    #[test]
    fn react_agent_matches_the_agent_trait() {
        let agent = crate::ReactAgent::new(
            crate::AgentDeps::new(
                std::sync::Arc::new(crate::MockModelClient),
                std::sync::Arc::new(crate::ToolRegistry::new()),
            ),
            3,
        );
        let _: &dyn crate::Agent = &agent;
        let _: &dyn crate::agent::agent_trait::Agent = &agent;
        fn assert_factory(_: crate::AgentFactory) {}
        let _ = assert_factory;
    }

    #[test]
    fn middleware_types_are_exported_at_crate_root() {
        fn assert_middleware(_: &dyn crate::Middleware) {}
        let _ = assert_middleware;
        fn assert_context(_: Option<&crate::HookCtx<'_>>) {}
        let _ = assert_context;
        fn assert_decision(_: crate::ToolDecision) {}
        let _ = assert_decision;
    }
}
