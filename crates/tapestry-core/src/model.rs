use async_trait::async_trait;

use crate::error::CoreError;
use crate::prompt::render_user;
use tapestry_protocol::{
    HistoryItem, ModelRequest, ModelResponse, TokenUsage, ToolCall, ToolCallId,
};

fn usage(input_tokens: u64, output_tokens: u64) -> TokenUsage {
    TokenUsage {
        input_tokens,
        output_tokens,
        ..Default::default()
    }
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
///
/// Environment context and system reminder items are not user messages and
/// are never echoed.
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
                usage: usage(8, 4),
            });
        };

        let rendered = render_user(name, content);
        let results: Vec<(&str, &str)> = request.history[idx + 1..]
            .iter()
            .filter_map(|item| match item {
                HistoryItem::ToolResult(result) => {
                    Some((result.tool_name.as_str(), result.output.as_str()))
                }
                _ => None,
            })
            .collect();

        if let Some(tool_name) = parse_tool_request(content) {
            if results.is_empty() {
                return Ok(ModelResponse {
                    content: None,
                    tool_calls: vec![ToolCall::new(
                        ToolCallId::generate(),
                        tool_name,
                        serde_json::json!({ "input": rendered }),
                    )],
                    usage: usage(12, 6),
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
                usage: usage(20, 10),
            });
        }

        Ok(ModelResponse {
            content: Some(format!("Echo: {rendered}")),
            tool_calls: vec![],
            usage: usage(8, 4),
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
    use tapestry_protocol::{EnvironmentContext, SystemReminder, ToolResult};

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
                    ToolResult::completed(&call, "hi").into(),
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
        let call = ToolCall::new(ToolCallId::generate(), "echo", serde_json::Value::Null);
        let response = MockModelClient
            .complete(request(
                vec![
                    HistoryItem::user(None, "tool:echo"),
                    HistoryItem::assistant(None, vec![call.clone()]),
                    ToolResult::completed(&call, "x").into(),
                    HistoryItem::assistant(Some("Done".into()), vec![]),
                    HistoryItem::user(None, "plain"),
                ],
                0,
            ))
            .await
            .unwrap();
        assert_eq!(response.content.as_deref(), Some("Echo: plain"));
    }

    #[tokio::test]
    async fn mock_does_not_echo_structured_items() {
        let response = MockModelClient
            .complete(request(
                vec![
                    HistoryItem::user(None, "question"),
                    EnvironmentContext::Unavailable.into(),
                    HistoryItem::system_reminder(SystemReminder::ToolLimitReached),
                ],
                1,
            ))
            .await
            .unwrap();
        assert_eq!(response.content.as_deref(), Some("Echo: question"));
    }
}
