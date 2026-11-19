use async_trait::async_trait;
use genai::adapter::AdapterKind;
use std::sync::Arc;
use tapestry_protocol::{ModelRequest, ModelResponse};

use super::ResponseModel;
use crate::error::CoreError;
use crate::formatter::Formatter;
use crate::model::genai::{GenaiModelClient, GenaiProviderConfig};
use crate::model::{ChatModel, ModelStream};

/// OpenAI Responses client (`/v1/responses`).
///
/// Each call sends the event snapshot from its request through the configured
/// formatter; server-side continuation via `previous_response_id` is not used.
#[derive(Clone)]
pub struct OpenAiResponseModel(GenaiModelClient);

impl OpenAiResponseModel {
    pub fn from_model(model: impl Into<String>) -> Result<Self, CoreError> {
        Self::from_config(&GenaiProviderConfig::new(model))
    }

    pub fn from_config(config: &GenaiProviderConfig) -> Result<Self, CoreError> {
        Ok(Self(GenaiModelClient::from_fixed_adapter(
            config,
            AdapterKind::OpenAIResp,
        )?))
    }

    /// Configure the event-to-message strategy used by every model call.
    pub fn with_formatter(self, formatter: Arc<dyn Formatter>) -> Self {
        Self(self.0.with_formatter(formatter))
    }
}

#[async_trait]
impl ResponseModel for OpenAiResponseModel {
    async fn respond(&self, request: ModelRequest) -> Result<ModelResponse, CoreError> {
        self.0.complete(request).await
    }

    async fn stream(&self, request: ModelRequest) -> Result<ModelStream, CoreError> {
        self.0.stream(request).await
    }
}
