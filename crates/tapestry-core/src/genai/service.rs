use genai::chat::{ChatOptions, ChatRequest, ChatStreamResponse};
use genai::Client;

use crate::error::CoreError;
use crate::genai::config::{build_client, GenaiProviderConfig};
use crate::genai::mapping::{chat_text_and_tokens, map_genai_error, simple_chat_request};

/// Result of a non-streaming chat turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenaiChatTurn {
    pub content: Option<String>,
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// Unified entry point for genai chat (non-streaming and streaming).
#[derive(Clone)]
pub struct GenaiChatService {
    client: Client,
    model: String,
}

impl GenaiChatService {
    /// Create a service from explicit provider configuration.
    pub fn from_config(config: &GenaiProviderConfig) -> Result<Self, CoreError> {
        let client = build_client(config)?;
        Ok(Self {
            client,
            model: config.model.clone(),
        })
    }

    /// Create a service using genai defaults and the given model name.
    pub fn from_model(model: impl Into<String>) -> Result<Self, CoreError> {
        Self::from_config(&GenaiProviderConfig::new(model))
    }

    /// Resolved model string used for requests.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Underlying genai client (for advanced use).
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Execute a non-streaming chat request.
    pub async fn exec_chat(
        &self,
        chat_req: ChatRequest,
        options: Option<&ChatOptions>,
    ) -> Result<GenaiChatTurn, CoreError> {
        let response = self
            .client
            .exec_chat(&self.model, chat_req, options)
            .await
            .map_err(map_genai_error)?;

        let (content, input_tokens, output_tokens) = chat_text_and_tokens(&response);
        Ok(GenaiChatTurn {
            content,
            input_tokens,
            output_tokens,
        })
    }

    /// Execute a streaming chat request.
    pub async fn exec_chat_stream(
        &self,
        chat_req: ChatRequest,
        options: Option<&ChatOptions>,
    ) -> Result<GenaiStreamResponse, CoreError> {
        let response = self
            .client
            .exec_chat_stream(&self.model, chat_req, options)
            .await
            .map_err(map_genai_error)?;
        Ok(GenaiStreamResponse { inner: response })
    }

    /// Convenience helper for a single system + user turn (non-streaming).
    pub async fn chat_simple(
        &self,
        system: Option<&str>,
        user: &str,
    ) -> Result<GenaiChatTurn, CoreError> {
        self.exec_chat(simple_chat_request(system, user), None)
            .await
    }

    /// Convenience helper for a single system + user turn (streaming).
    pub async fn chat_simple_stream(
        &self,
        system: Option<&str>,
        user: &str,
    ) -> Result<GenaiStreamResponse, CoreError> {
        self.exec_chat_stream(simple_chat_request(system, user), None)
            .await
    }
}

/// Streaming chat response wrapper (hides genai stream type from callers).
pub struct GenaiStreamResponse {
    pub(crate) inner: ChatStreamResponse,
}

impl GenaiStreamResponse {
    /// Access the underlying genai stream response.
    pub fn into_inner(self) -> ChatStreamResponse {
        self.inner
    }
}
