use async_trait::async_trait;
use genai::adapter::AdapterKind;
use std::sync::Arc;
use tapestry_protocol::{ModelRequest, ModelResponse};

use super::ChatModel;
use crate::error::CoreError;
use crate::formatter::Formatter;
use crate::model::genai::{GenaiModelClient, GenaiProviderConfig};
use crate::model::ModelStream;

/// OpenAI Chat Completions client (`/v1/chat/completions`).
#[derive(Clone)]
pub struct OpenAiChatModel(GenaiModelClient);

impl OpenAiChatModel {
    pub fn from_model(model: impl Into<String>) -> Result<Self, CoreError> {
        Self::from_config(&GenaiProviderConfig::new(model))
    }

    pub fn from_config(config: &GenaiProviderConfig) -> Result<Self, CoreError> {
        Ok(Self(GenaiModelClient::from_fixed_adapter(
            config,
            AdapterKind::OpenAI,
        )?))
    }

    /// Configure the event-to-message strategy used by every model call.
    ///
    /// ```no_run
    /// use std::sync::Arc;
    /// use tapestry_core::{CoreError, OpenAiChatModel, OpenAiFormatter};
    ///
    /// fn build() -> Result<OpenAiChatModel, CoreError> {
    ///     Ok(OpenAiChatModel::from_model("gpt-4o-mini")?
    ///         .with_formatter(Arc::new(OpenAiFormatter)))
    /// }
    /// ```
    pub fn with_formatter(self, formatter: Arc<dyn Formatter>) -> Self {
        Self(self.0.with_formatter(formatter))
    }
}

#[async_trait]
impl ChatModel for OpenAiChatModel {
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, CoreError> {
        self.0.complete(request).await
    }

    async fn stream(&self, request: ModelRequest) -> Result<ModelStream, CoreError> {
        self.0.stream(request).await
    }
}
