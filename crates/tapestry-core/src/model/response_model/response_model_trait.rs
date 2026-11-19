use async_trait::async_trait;
use tapestry_protocol::{ModelRequest, ModelResponse};

use crate::error::CoreError;

use crate::model::ModelStream;

/// Response-style model invocation using Tapestry's normalized model types.
#[async_trait]
pub trait ResponseModel: Send + Sync {
    async fn respond(&self, request: ModelRequest) -> Result<ModelResponse, CoreError>;

    /// Streamed variant of [`respond`](Self::respond). The default yields
    /// only the final response.
    async fn stream(&self, request: ModelRequest) -> Result<ModelStream, CoreError> {
        Ok(ModelStream::completed(self.respond(request).await?))
    }
}

#[cfg(test)]
mod tests {
    use tapestry_protocol::ModelStreamItem;

    use super::*;

    struct RespondOnly;

    #[async_trait]
    impl ResponseModel for RespondOnly {
        async fn respond(&self, _request: ModelRequest) -> Result<ModelResponse, CoreError> {
            Ok(ModelResponse {
                content: Some("response".into()),
                tool_calls: vec![],
                usage: Default::default(),
            })
        }
    }

    #[tokio::test]
    async fn default_stream_yields_only_the_response() {
        let request = ModelRequest {
            system_prompt: None,
            events: vec![],
            tools: vec![],
            iteration: 0,
        };
        let model: &dyn ResponseModel = &RespondOnly;
        let response = model.respond(request.clone()).await.unwrap();
        let mut stream = model.stream(request).await.unwrap();
        assert_eq!(
            stream.next().await.unwrap().unwrap(),
            ModelStreamItem::Completed { response }
        );
        assert!(stream.next().await.is_none());
    }
}
