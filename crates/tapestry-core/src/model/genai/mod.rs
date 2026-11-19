//! Multi-provider LLM access via the [`genai`](https://crates.io/crates/genai) crate.
//!
//! This integration is included by `tapestry-core` by default.

mod config;
mod mapping;
mod model_client;
mod service;

pub use config::{
    build_client, default_api_key_env, parse_model_name, GenaiProviderConfig, ParsedModelName,
};
pub use mapping::model_request_to_chat_request;
pub use model_client::GenaiModelClient;
pub use service::{GenaiChatService, GenaiChatTurn, GenaiStreamResponse};
