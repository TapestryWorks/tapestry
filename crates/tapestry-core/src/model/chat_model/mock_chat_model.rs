use async_trait::async_trait;
use tapestry_protocol::{
    ConversationItem, ModelRequest, ModelResponse, ModelStreamItem, TokenUsage, ToolCall,
    ToolCallId,
};

use crate::error::CoreError;
use crate::formatter::prompt::render_user;
use crate::formatter::{Formatter, OpenAiFormatter};

use crate::model::ModelStream;

use super::ChatModel;

fn usage(input_tokens: u64, output_tokens: u64) -> TokenUsage {
    TokenUsage {
        input_tokens,
        output_tokens,
        ..Default::default()
    }
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
pub struct MockChatModel;

#[async_trait]
impl ChatModel for MockChatModel {
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, CoreError> {
        let history = OpenAiFormatter.events_to_items(&request.events);
        let Some((idx, name, content)) =
            history
                .iter()
                .enumerate()
                .rev()
                .find_map(|(idx, item)| match item {
                    ConversationItem::User { name, content } => {
                        Some((idx, name.as_deref(), content))
                    }
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
        let results: Vec<(&str, &str)> = history[idx + 1..]
            .iter()
            .filter_map(|item| match item {
                ConversationItem::ToolResult(result) => {
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

    /// Streams the text of [`complete`](Self::complete) word by word (each
    /// delta keeps its trailing whitespace), then the full response.
    async fn stream(&self, request: ModelRequest) -> Result<ModelStream, CoreError> {
        let response = self.complete(request).await?;
        let mut items: Vec<_> = response
            .content
            .as_deref()
            .unwrap_or_default()
            .split_inclusive(char::is_whitespace)
            .map(|word| {
                Ok(ModelStreamItem::TextDelta {
                    text: word.to_string(),
                })
            })
            .collect();
        items.push(Ok(ModelStreamItem::Completed { response }));
        Ok(ModelStream::from_items(items))
    }
}

fn parse_tool_request(message: &str) -> Option<String> {
    message
        .split_whitespace()
        .find_map(|token| token.strip_prefix("tool:").map(str::to_string))
}

#[cfg(test)]
mod tests {
    use tapestry_protocol::{
        AssistantMessageEvent, ConversationItem, EnvironmentContext,
        EnvironmentContextUpdatedEvent, Event, EventMsg, MessageId, SubmissionId, SystemReminder,
        SystemReminderEvent, ThreadId, ToolCallEndEvent, ToolResult, TurnId, UserMessageEvent,
    };

    use super::*;

    fn request(items: Vec<ConversationItem>, iteration: u32) -> ModelRequest {
        let submission_id = SubmissionId::new();
        let thread_id = ThreadId::new();
        let turn_id = TurnId::new();
        let events = items
            .into_iter()
            .enumerate()
            .map(|(index, item)| {
                let msg = match item {
                    ConversationItem::User { name, content } => {
                        EventMsg::UserMessage(UserMessageEvent { name, content })
                    }
                    ConversationItem::Assistant {
                        content,
                        tool_calls,
                    } => EventMsg::AssistantMessage(AssistantMessageEvent {
                        message_id: MessageId::new(),
                        content,
                        tool_calls,
                    }),
                    ConversationItem::ToolResult(result) => {
                        EventMsg::ToolCallEnd(ToolCallEndEvent {
                            result,
                            duration_ms: None,
                        })
                    }
                    ConversationItem::EnvironmentContext(context) => {
                        EventMsg::EnvironmentContextUpdated(EnvironmentContextUpdatedEvent {
                            context,
                        })
                    }
                    ConversationItem::SystemReminder(reminder) => {
                        EventMsg::SystemReminder(SystemReminderEvent { reminder })
                    }
                };
                Event {
                    id: submission_id,
                    log_position: index as u64 + 1,
                    thread_id,
                    turn_id: Some(turn_id),
                    timestamp_ms: index as u64,
                    msg,
                }
            })
            .collect();
        ModelRequest {
            system_prompt: None,
            events,
            tools: vec![],
            iteration,
        }
    }

    async fn drain(mut stream: ModelStream) -> Vec<Result<ModelStreamItem, CoreError>> {
        let mut items = Vec::new();
        while let Some(item) = stream.next().await {
            items.push(item);
        }
        items
    }

    #[tokio::test]
    async fn mock_stream_deltas_concatenate_to_complete_text() {
        let req = request(vec![ConversationItem::user(None, "hello  big world")], 0);
        let expected = MockChatModel.complete(req.clone()).await.unwrap();
        let items = drain(MockChatModel.stream(req).await.unwrap()).await;
        let (last, deltas) = items.split_last().unwrap();
        assert!(deltas.len() > 1);
        let text: String = deltas
            .iter()
            .map(|item| match item {
                Ok(ModelStreamItem::TextDelta { text }) => text.as_str(),
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(Some(text), expected.content.clone());
        assert_eq!(
            last.as_ref().unwrap(),
            &ModelStreamItem::Completed { response: expected }
        );
    }

    #[tokio::test]
    async fn mock_stream_without_text_has_no_deltas() {
        let req = request(vec![ConversationItem::user(None, "tool:echo")], 0);
        let items = drain(MockChatModel.stream(req).await.unwrap()).await;
        assert_eq!(items.len(), 1);
        assert!(matches!(items[0], Ok(ModelStreamItem::Completed { .. })));
    }

    #[tokio::test]
    async fn mock_echoes_plain_message() {
        let response = MockChatModel
            .complete(request(vec![ConversationItem::user(None, "hello")], 0))
            .await
            .unwrap();

        assert_eq!(response.content.as_deref(), Some("Echo: hello"));
        assert!(response.tool_calls.is_empty());
    }

    #[tokio::test]
    async fn mock_uses_name_prefix_for_named_user() {
        let response = MockChatModel
            .complete(request(
                vec![ConversationItem::user(Some("alice".into()), "hi")],
                0,
            ))
            .await
            .unwrap();
        assert_eq!(response.content.as_deref(), Some("Echo: alice: hi"));
    }

    #[tokio::test]
    async fn mock_requests_tool_then_summarizes() {
        let user = ConversationItem::user(None, "please tool:echo now");

        let first = MockChatModel
            .complete(request(vec![user.clone()], 0))
            .await
            .unwrap();
        assert_eq!(first.tool_calls.len(), 1);
        assert_eq!(first.tool_calls[0].name, "echo");

        let call = first.tool_calls[0].clone();
        let second = MockChatModel
            .complete(request(
                vec![
                    user,
                    ConversationItem::assistant(None, vec![call.clone()]),
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
        let response = MockChatModel
            .complete(request(
                vec![
                    ConversationItem::user(None, "tool:echo"),
                    ConversationItem::assistant(None, vec![call.clone()]),
                    ToolResult::completed(&call, "x").into(),
                    ConversationItem::assistant(Some("Done".into()), vec![]),
                    ConversationItem::user(None, "plain"),
                ],
                0,
            ))
            .await
            .unwrap();
        assert_eq!(response.content.as_deref(), Some("Echo: plain"));
    }

    #[tokio::test]
    async fn mock_does_not_echo_structured_items() {
        let response = MockChatModel
            .complete(request(
                vec![
                    ConversationItem::user(None, "question"),
                    EnvironmentContext::Unavailable.into(),
                    ConversationItem::system_reminder(SystemReminder::ToolLimitReached),
                ],
                1,
            ))
            .await
            .unwrap();
        assert_eq!(response.content.as_deref(), Some("Echo: question"));
    }
}
