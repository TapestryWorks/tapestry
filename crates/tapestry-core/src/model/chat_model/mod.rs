//! Chat model interface and implementations.

mod chat_model_trait;
mod mock_chat_model;
mod openai_chat_model;

pub use chat_model_trait::ChatModel;
pub use mock_chat_model::MockChatModel;
pub use openai_chat_model::OpenAiChatModel;
