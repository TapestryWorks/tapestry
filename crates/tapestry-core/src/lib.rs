//! Tapestry agent core: thread lifecycle, agent loop, and model interaction.
//!
//! Architecture is informed by OpenAI Codex `codex-core`; see `docs/architecture.md`.

pub mod agent;
pub mod clock;
pub mod config;
pub mod context;
pub mod error;
pub mod history;
pub mod model;
pub mod prompt;
pub mod thread;
pub mod tool;
pub mod turn;

#[cfg(feature = "genai")]
pub mod genai;

pub use agent::AgentLoop;
pub use clock::{Clock, ClockError, ClockReading, FixedClock, SystemClock};
pub use config::ThreadConfig;
pub use error::CoreError;
pub use history::fill_missing_tool_results;
pub use model::{MockModelClient, ModelClient};
pub use prompt::{
    render_environment_context, render_system_reminder, render_user, FORCED_FINISH_REMINDER,
};
pub use tapestry_protocol::{
    EnvironmentContext, HistoryItem, ModelOutputEffect, ModelRequest, ModelResponse,
    SystemReminder, TokenUsage, ToolCallId, ToolResult, ToolSpec, TurnStep,
};

#[cfg(feature = "genai")]
pub use genai::{
    default_api_key_env, parse_model_name, GenaiChatService, GenaiModelClient, GenaiProviderConfig,
};
pub use thread::{TapestryThread, ThreadHandle};
pub use tool::{EchoTool, Tool, ToolRegistry};
pub use turn::TurnMachine;
