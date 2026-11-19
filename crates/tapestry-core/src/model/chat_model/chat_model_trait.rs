use async_trait::async_trait;
use tapestry_protocol::{ModelRequest, ModelResponse};

use crate::error::CoreError;

use crate::model::ModelStream;

/// Produces a chat completion from the event snapshot in a request.
#[async_trait]
pub trait ChatModel: Send + Sync {
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, CoreError>;

    /// Streamed variant of [`complete`](Self::complete). The default yields
    /// only the final response, so non-streaming models need no changes.
    async fn stream(&self, request: ModelRequest) -> Result<ModelStream, CoreError> {
        Ok(ModelStream::completed(self.complete(request).await?))
    }
}

#[cfg(test)]
mod tests {
    use tapestry_protocol::ModelStreamItem;

    use super::*;

    struct CompleteOnly;

    #[async_trait]
    impl ChatModel for CompleteOnly {
        async fn complete(&self, _request: ModelRequest) -> Result<ModelResponse, CoreError> {
            Ok(ModelResponse {
                content: Some("whole".into()),
                tool_calls: vec![],
                usage: Default::default(),
            })
        }
    }

    #[tokio::test]
    async fn default_stream_yields_only_the_complete_response() {
        let request = ModelRequest {
            system_prompt: None,
            events: vec![],
            tools: vec![],
            iteration: 0,
        };
        let response = CompleteOnly.complete(request.clone()).await.unwrap();
        let mut stream = CompleteOnly.stream(request).await.unwrap();
        assert_eq!(
            stream.next().await.unwrap().unwrap(),
            ModelStreamItem::Completed { response }
        );
        assert!(stream.next().await.is_none());
    }
}
