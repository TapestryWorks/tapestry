//! Tapestry agent core: thread lifecycle, agent loop, and model interaction.
//!
//! Architecture is informed by OpenAI Codex `codex-core`; see `docs/architecture.md`.

pub mod agent;
pub mod clock;
pub mod config;
pub mod context;
pub mod error;
pub mod formatter;
pub mod model;
pub mod projection;
pub mod recorder;
pub mod store;
pub mod thread;
pub mod tool;
pub mod turn;

pub mod genai;

pub use agent::AgentLoop;
pub use clock::{Clock, ClockError, ClockReading, FixedClock, SystemClock};
pub use config::ThreadConfig;
pub use error::CoreError;
pub use formatter::prompt::{
    render_environment_context, render_system_reminder, render_user, FORCED_FINISH_REMINDER,
};
pub use formatter::Formatter;
pub use formatter::OpenAiFormatter;
pub use model::{
    ChatModel, MockChatModel, MockModelClient, MockResponseModel, ModelClient, ModelStream,
    ModelStreamSource, ResponseModel,
};
pub use projection::HistoryProjection;
pub use tapestry_protocol::{
    EnvironmentContext, HistoryItem, ModelRequest, ModelResponse, SystemReminder, TokenUsage,
    ToolCallId, ToolResult, ToolSpec,
};

pub use genai::{
    default_api_key_env, parse_model_name, GenaiChatService, GenaiModelClient, GenaiProviderConfig,
};
pub use model::{OpenAiChatModel, OpenAiResponseModel};
pub use recorder::Recorder;
pub use store::{EventStore, EventSubscription, InMemoryEventStore, NewEvent, SubscriptionSource};
pub use thread::{TapestryThread, ThreadBuilder, ThreadHandle};
pub use tool::{EchoTool, Tool, ToolRegistry};
pub use turn::{ModelOutputEffect, TurnMachine, TurnStep};

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

        let _: crate::projection::HistoryProjection =
            crate::formatter::formatter_trait::HistoryProjection::new();
        assert_eq!(
            crate::formatter::prompt::render_user(None, "hello"),
            "hello"
        );
    }

    #[test]
    fn legacy_genai_path_remains_available() {
        fn assert_type<T>() {}
        assert_type::<crate::genai::GenaiProviderConfig>();
        assert_type::<crate::genai::GenaiModelClient>();
    }
}
