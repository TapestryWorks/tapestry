use async_trait::async_trait;
use serde_json::Value;

use crate::error::CoreError;

/// A tool invocation requested by the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelToolCall {
    pub name: String,
    pub arguments: Value,
}

/// Outcome of a single model completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<ModelToolCall>,
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// Conversation state passed to the model client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRequest {
    pub system_prompt: Option<String>,
    pub user_message: String,
    pub tool_results: Vec<(String, String)>,
    pub iteration: u32,
}

#[async_trait]
pub trait ModelClient: Send + Sync {
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, CoreError>;
}

/// Deterministic mock model for tests and local development.
///
/// Behavior:
/// - First iteration: if the user message contains `tool:NAME`, request that tool.
/// - After tool results are present: respond with a summary message.
/// - Otherwise: echo the user message.
#[derive(Debug, Default)]
pub struct MockModelClient;

#[async_trait]
impl ModelClient for MockModelClient {
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, CoreError> {
        if let Some(tool_name) = parse_tool_request(&request.user_message) {
            if request.tool_results.is_empty() {
                return Ok(ModelResponse {
                    content: None,
                    tool_calls: vec![ModelToolCall {
                        name: tool_name.clone(),
                        arguments: serde_json::json!({
                            "input": request.user_message,
                        }),
                    }],
                    input_tokens: 12,
                    output_tokens: 6,
                });
            }

            let summary = request
                .tool_results
                .iter()
                .map(|(name, output)| format!("{name}={output}"))
                .collect::<Vec<_>>()
                .join(", ");

            return Ok(ModelResponse {
                content: Some(format!("Done. Tool results: {summary}")),
                tool_calls: vec![],
                input_tokens: 20,
                output_tokens: 10,
            });
        }

        Ok(ModelResponse {
            content: Some(format!("Echo: {}", request.user_message)),
            tool_calls: vec![],
            input_tokens: 8,
            output_tokens: 4,
        })
    }
}

fn parse_tool_request(message: &str) -> Option<String> {
    message
        .split_whitespace()
        .find_map(|token| token.strip_prefix("tool:").map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_echoes_plain_message() {
        let client = MockModelClient;
        let response = client
            .complete(ModelRequest {
                system_prompt: None,
                user_message: "hello".into(),
                tool_results: vec![],
                iteration: 0,
            })
            .await
            .unwrap();

        assert_eq!(response.content.as_deref(), Some("Echo: hello"));
        assert!(response.tool_calls.is_empty());
    }

    #[tokio::test]
    async fn mock_requests_tool_then_summarizes() {
        let client = MockModelClient;

        let first = client
            .complete(ModelRequest {
                system_prompt: None,
                user_message: "please tool:echo now".into(),
                tool_results: vec![],
                iteration: 0,
            })
            .await
            .unwrap();
        assert_eq!(first.tool_calls.len(), 1);
        assert_eq!(first.tool_calls[0].name, "echo");

        let second = client
            .complete(ModelRequest {
                system_prompt: None,
                user_message: "please tool:echo now".into(),
                tool_results: vec![("echo".into(), "hi".into())],
                iteration: 1,
            })
            .await
            .unwrap();
        assert!(second.tool_calls.is_empty());
        assert!(second.content.unwrap().contains("echo=hi"));
    }
}
