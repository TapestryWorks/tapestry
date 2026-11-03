use async_trait::async_trait;

use crate::error::CoreError;
use crate::history::{render_user_content, HistoryItem, ToolCall};
use crate::tool::ToolSpec;

/// Outcome of a single model completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// Conversation state passed to the model client.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelRequest {
    pub system_prompt: Option<String>,
    pub history: Vec<HistoryItem>,
    /// Empty when tools are not allowed for this call.
    pub tools: Vec<ToolSpec>,
    pub iteration: u32,
}

#[async_trait]
pub trait ModelClient: Send + Sync {
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, CoreError>;
}

/// Deterministic mock model for tests and local development.
///
/// Behavior, based on the most recent user message in the history:
/// - If it contains `tool:NAME` and no tool result follows it, request that tool.
/// - If tool results follow it, respond with a summary message.
/// - Otherwise: echo the (rendered) user message.
#[derive(Debug, Default)]
pub struct MockModelClient;

#[async_trait]
impl ModelClient for MockModelClient {
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, CoreError> {
        let Some((idx, name, content)) =
            request
                .history
                .iter()
                .enumerate()
                .rev()
                .find_map(|(idx, item)| match item {
                    HistoryItem::User { name, content } => Some((idx, name.as_deref(), content)),
                    _ => None,
                })
        else {
            return Ok(ModelResponse {
                content: Some("Echo: ".into()),
                tool_calls: vec![],
                input_tokens: 8,
                output_tokens: 4,
            });
        };

        let rendered = render_user_content(name, content);
        let results: Vec<(&str, &str)> = request.history[idx + 1..]
            .iter()
            .filter_map(|item| match item {
                HistoryItem::ToolResult {
                    tool_name, output, ..
                } => Some((tool_name.as_str(), output.as_str())),
                _ => None,
            })
            .collect();

        if let Some(tool_name) = parse_tool_request(content) {
            if results.is_empty() {
                return Ok(ModelResponse {
                    content: None,
                    tool_calls: vec![ToolCall::new(
                        tool_name,
                        serde_json::json!({ "input": rendered }),
                    )],
                    input_tokens: 12,
                    output_tokens: 6,
                });
            }

            let summary = results
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
            content: Some(format!("Echo: {rendered}")),
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
    use tapestry_protocol::ToolCallStatus;

    use super::*;

    fn request(history: Vec<HistoryItem>, iteration: u32) -> ModelRequest {
        ModelRequest {
            system_prompt: None,
            history,
            tools: vec![],
            iteration,
        }
    }

    #[tokio::test]
    async fn mock_echoes_plain_message() {
        let response = MockModelClient
            .complete(request(vec![HistoryItem::user(None, "hello")], 0))
            .await
            .unwrap();

        assert_eq!(response.content.as_deref(), Some("Echo: hello"));
        assert!(response.tool_calls.is_empty());
    }

    #[tokio::test]
    async fn mock_uses_name_prefix_for_named_user() {
        let response = MockModelClient
            .complete(request(
                vec![HistoryItem::user(Some("alice".into()), "hi")],
                0,
            ))
            .await
            .unwrap();
        assert_eq!(response.content.as_deref(), Some("Echo: alice: hi"));
    }

    #[tokio::test]
    async fn mock_requests_tool_then_summarizes() {
        let user = HistoryItem::user(None, "please tool:echo now");

        let first = MockModelClient
            .complete(request(vec![user.clone()], 0))
            .await
            .unwrap();
        assert_eq!(first.tool_calls.len(), 1);
        assert_eq!(first.tool_calls[0].name, "echo");

        let call = first.tool_calls[0].clone();
        let second = MockModelClient
            .complete(request(
                vec![
                    user,
                    HistoryItem::assistant(None, vec![call.clone()]),
                    HistoryItem::tool_result(call.id, "echo", "hi", ToolCallStatus::Completed),
                ],
                1,
            ))
            .await
            .unwrap();
        assert!(second.tool_calls.is_empty());
        assert!(second.content.unwrap().contains("echo=hi"));
    }

    #[tokio::test]
    async fn mock_ignores_tool_results_of_earlier_turns() {
        let call = ToolCall::new("echo", serde_json::Value::Null);
        let response = MockModelClient
            .complete(request(
                vec![
                    HistoryItem::user(None, "tool:echo"),
                    HistoryItem::assistant(None, vec![call.clone()]),
                    HistoryItem::tool_result(call.id, "echo", "x", ToolCallStatus::Completed),
                    HistoryItem::assistant(Some("Done".into()), vec![]),
                    HistoryItem::user(None, "plain"),
                ],
                0,
            ))
            .await
            .unwrap();
        assert_eq!(response.content.as_deref(), Some("Echo: plain"));
    }
}
