use async_trait::async_trait;

use crate::error::CoreError;
use crate::genai::config::GenaiProviderConfig;
use crate::genai::mapping::{
    chat_response_to_model_response, map_genai_error, model_request_to_chat_request,
};
use crate::genai::service::GenaiChatService;
use crate::model::{ModelClient, ModelRequest, ModelResponse};

/// [`ModelClient`] backed by the [`genai`](https://crates.io/crates/genai) crate.
#[derive(Clone)]
pub struct GenaiModelClient {
    service: GenaiChatService,
}

impl GenaiModelClient {
    pub fn from_config(config: &GenaiProviderConfig) -> Result<Self, CoreError> {
        Ok(Self {
            service: GenaiChatService::from_config(config)?,
        })
    }

    pub fn from_model(model: impl Into<String>) -> Result<Self, CoreError> {
        Ok(Self {
            service: GenaiChatService::from_model(model)?,
        })
    }

    pub fn service(&self) -> &GenaiChatService {
        &self.service
    }
}

#[async_trait]
impl ModelClient for GenaiModelClient {
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, CoreError> {
        let chat_req = model_request_to_chat_request(&request);
        let response = self
            .service
            .client()
            .exec_chat(self.service.model(), chat_req, None)
            .await
            .map_err(map_genai_error)?;
        Ok(chat_response_to_model_response(response))
    }
}
