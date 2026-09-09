//! Tapestry agent core: thread lifecycle, agent loop, and model interaction.
//!
//! Architecture is informed by OpenAI Codex `codex-core`; see `docs/architecture.md`.

pub mod agent;
pub mod config;
pub mod error;
pub mod model;
pub mod thread;
pub mod tool;

#[cfg(feature = "genai")]
pub mod genai;

pub use agent::AgentLoop;
pub use config::ThreadConfig;
pub use error::CoreError;
pub use model::{MockModelClient, ModelClient, ModelRequest, ModelResponse, ModelToolCall};

#[cfg(feature = "genai")]
pub use genai::{
    default_api_key_env, parse_model_name, GenaiChatService, GenaiModelClient, GenaiProviderConfig,
};
pub use thread::{TapestryThread, ThreadHandle};
pub use tool::{EchoTool, Tool, ToolContext, ToolRegistry, ToolResult};
