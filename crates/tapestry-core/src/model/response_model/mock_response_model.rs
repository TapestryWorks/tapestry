use async_trait::async_trait;
use tapestry_protocol::{ModelRequest, ModelResponse};

use crate::error::CoreError;

use crate::model::chat_model::{ChatModel, MockChatModel};
use crate::model::ModelStream;

use super::ResponseModel;

/// Response-model mock with the same echo and tool behavior as [`MockChatModel`].
#[derive(Debug, Default)]
pub struct MockResponseModel;

#[async_trait]
impl ResponseModel for MockResponseModel {
    async fn respond(&self, request: ModelRequest) -> Result<ModelResponse, CoreError> {
        MockChatModel.complete(request).await
    }

    async fn stream(&self, request: ModelRequest) -> Result<ModelStream, CoreError> {
        MockChatModel.stream(request).await
    }
}

#[cfg(test)]
mod tests {
    use tapestry_protocol::{
        AssistantMessageEvent, EnvironmentContextUpdatedEvent, Event, EventMsg, HistoryItem,
        MessageId, ModelStreamItem, SubmissionId, SystemReminderEvent, ThreadId, ToolCallEndEvent,
        ToolResult, TurnId, UserMessageEvent,
    };

    use super::*;

    fn request(items: Vec<HistoryItem>, iteration: u32) -> ModelRequest {
        let submission_id = SubmissionId::new();
        let thread_id = ThreadId::new();
        let turn_id = TurnId::new();
        let events = items
            .into_iter()
            .enumerate()
            .map(|(index, item)| {
                let msg = match item {
                    HistoryItem::User { name, content } => {
                        EventMsg::UserMessage(UserMessageEvent { name, content })
                    }
                    HistoryItem::Assistant {
                        content,
                        tool_calls,
                    } => EventMsg::AssistantMessage(AssistantMessageEvent {
                        message_id: MessageId::new(),
                        content,
                        tool_calls,
                    }),
                    HistoryItem::ToolResult(result) => EventMsg::ToolCallEnd(ToolCallEndEvent {
                        result,
                        duration_ms: None,
                    }),
                    HistoryItem::EnvironmentContext(context) => {
                        EventMsg::EnvironmentContextUpdated(EnvironmentContextUpdatedEvent {
                            context,
                        })
                    }
                    HistoryItem::SystemReminder(reminder) => {
                        EventMsg::SystemReminder(SystemReminderEvent {
                            iteration: 0,
                            reminder,
                        })
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

    #[tokio::test]
    async fn mock_response_streams_echo_and_final_response() {
        let req = request(vec![HistoryItem::user(None, "hello  world")], 0);
        let expected = MockResponseModel.respond(req.clone()).await.unwrap();
        let mut stream = MockResponseModel.stream(req).await.unwrap();
        let mut text = String::new();
        let mut deltas = 0;
        let response = loop {
            match stream.next().await.unwrap().unwrap() {
                ModelStreamItem::TextDelta { text: delta } => {
                    text.push_str(&delta);
                    deltas += 1;
                }
                ModelStreamItem::Completed { response } => break response,
            }
        };
        assert!(deltas > 1);
        assert_eq!(text, "Echo: hello  world");
        assert_eq!(response, expected);
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn mock_response_streams_tool_call_without_text_then_summarizes_result() {
        let user = HistoryItem::user(None, "tool:echo");
        let mut stream = MockResponseModel
            .stream(request(vec![user.clone()], 0))
            .await
            .unwrap();
        let ModelStreamItem::Completed { response } = stream.next().await.unwrap().unwrap() else {
            panic!("expected completion without text deltas");
        };
        assert!(stream.next().await.is_none());
        assert!(response.content.is_none());
        let [call] = response.tool_calls.as_slice() else {
            panic!("expected exactly one tool call");
        };
        assert_eq!(call.name, "echo");

        let response = MockResponseModel
            .respond(request(
                vec![
                    user,
                    HistoryItem::assistant(None, vec![call.clone()]),
                    ToolResult::completed(call, "hi").into(),
                ],
                1,
            ))
            .await
            .unwrap();
        assert_eq!(
            response.content.as_deref(),
            Some("Done. Tool results: echo=hi")
        );
        assert!(response.tool_calls.is_empty());
    }
}
