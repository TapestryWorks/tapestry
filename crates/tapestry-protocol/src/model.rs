//! Backend-neutral model protocol: what a model client receives and returns.

use serde::{Deserialize, Serialize};

use crate::common::{EnvironmentContext, SystemReminder, TokenUsage};
use crate::event::Event;
use crate::tool::{ToolCall, ToolResult, ToolSpec};

/// One entry of the conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConversationItem {
    /// Input from a user-side speaker; `name` identifies the speaker.
    User {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        content: String,
    },
    /// Output of the agent itself. Never carries a name.
    Assistant {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCall>,
    },
    ToolResult(ToolResult),
    EnvironmentContext(EnvironmentContext),
    /// Projected from a recorded reminder event that no model request has consumed yet.
    SystemReminder(SystemReminder),
}

impl ConversationItem {
    pub fn user(name: Option<String>, content: impl Into<String>) -> Self {
        Self::User {
            name,
            content: content.into(),
        }
    }

    pub fn assistant(content: Option<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self::Assistant {
            content,
            tool_calls,
        }
    }

    pub fn system_reminder(reminder: SystemReminder) -> Self {
        Self::SystemReminder(reminder)
    }
}

impl From<ToolResult> for ConversationItem {
    fn from(result: ToolResult) -> Self {
        Self::ToolResult(result)
    }
}

impl From<EnvironmentContext> for ConversationItem {
    fn from(context: EnvironmentContext) -> Self {
        Self::EnvironmentContext(context)
    }
}

/// Conversation events passed to the model client in log order.
///
/// ```no_run
/// use tapestry_protocol::{Event, ModelRequest};
///
/// fn request(events: Vec<Event>) -> ModelRequest {
///     ModelRequest {
///         system_prompt: None,
///         events,
///         tools: Vec::new(),
///         iteration: 0,
///     }
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRequest {
    pub system_prompt: Option<String>,
    /// Ordered, immutable snapshot of the thread log at the start of this call.
    pub events: Vec<Event>,
    /// Empty when tools are not allowed for this call.
    pub tools: Vec<ToolSpec>,
    pub iteration: u32,
}

/// Outcome of a single model completion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub usage: TokenUsage,
}

/// One item of a streamed model call: text deltas, then exactly one final
/// `Completed` carrying the full response (tool calls and usage included).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelStreamItem {
    TextDelta { text: String },
    Completed { response: ModelResponse },
}

#[cfg(test)]
mod tests {
    use crate::event::{
        AssistantMessageEvent, EventMsg, SystemReminderEvent, ToolCallEndEvent, UserMessageEvent,
    };
    use serde_json::json;

    use super::*;

    fn call() -> ToolCall {
        ToolCall::new("call_1".into(), "echo", json!({"input": "x"}))
    }

    #[test]
    fn items_round_trip() {
        let call = call();
        let items = vec![
            ConversationItem::user(Some("alice".into()), "hi"),
            ConversationItem::assistant(None, vec![call.clone()]),
            ToolResult::completed(&call, "x").into(),
            ConversationItem::assistant(Some("done".into()), vec![]),
            EnvironmentContext::Available {
                date: "2026-11-05".into(),
                timezone: "UTC".into(),
            }
            .into(),
            EnvironmentContext::Unavailable.into(),
            ConversationItem::system_reminder(SystemReminder::ToolLimitReached),
            ConversationItem::system_reminder(SystemReminder::custom("be brief")),
        ];
        let text = serde_json::to_string(&items).unwrap();
        let back: Vec<ConversationItem> = serde_json::from_str(&text).unwrap();
        assert_eq!(back, items);
    }

    #[test]
    fn wire_format_is_tagged_and_omits_empty_fields() {
        let user = serde_json::to_value(ConversationItem::user(None, "hi")).unwrap();
        assert_eq!(user, json!({"type": "user", "content": "hi"}));

        let assistant =
            serde_json::to_value(ConversationItem::assistant(Some("ok".into()), vec![])).unwrap();
        assert_eq!(assistant, json!({"type": "assistant", "content": "ok"}));

        let parsed: ConversationItem =
            serde_json::from_value(json!({"type": "assistant"})).unwrap();
        assert_eq!(parsed, ConversationItem::assistant(None, vec![]));
    }

    #[test]
    fn tool_result_wire_format_is_flat() {
        let value =
            serde_json::to_value(ConversationItem::from(ToolResult::not_found(&call()))).unwrap();
        assert_eq!(
            value,
            json!({
                "type": "tool_result",
                "call_id": "call_1",
                "tool_name": "echo",
                "output": "tool `echo` not found",
                "status": "not_found"
            })
        );
    }

    #[test]
    fn environment_context_wire_format() {
        let available = ConversationItem::from(EnvironmentContext::Available {
            date: "2026-11-05".into(),
            timezone: "Asia/Shanghai".into(),
        });
        assert_eq!(
            serde_json::to_value(available).unwrap(),
            json!({
                "type": "environment_context",
                "state": "available",
                "date": "2026-11-05",
                "timezone": "Asia/Shanghai"
            })
        );
        assert_eq!(
            serde_json::to_value(ConversationItem::from(EnvironmentContext::Unavailable)).unwrap(),
            json!({"type": "environment_context", "state": "unavailable"})
        );
    }

    #[test]
    fn system_reminder_wire_format() {
        assert_eq!(
            serde_json::to_value(ConversationItem::system_reminder(
                SystemReminder::ToolLimitReached
            ))
            .unwrap(),
            json!({"type": "system_reminder", "reminder": "tool_limit_reached"})
        );
    }

    #[test]
    fn custom_system_reminder_wire_format() {
        assert_eq!(
            serde_json::to_value(ConversationItem::system_reminder(SystemReminder::custom(
                "be brief"
            )))
            .unwrap(),
            json!({"type": "system_reminder", "reminder": "custom", "text": "be brief"})
        );
    }

    #[test]
    fn stream_items_round_trip_with_snake_case_tags() {
        let delta = ModelStreamItem::TextDelta { text: "Hel".into() };
        assert_eq!(
            serde_json::to_value(&delta).unwrap(),
            json!({"type": "text_delta", "text": "Hel"})
        );
        let completed = ModelStreamItem::Completed {
            response: ModelResponse {
                content: Some("Hello".into()),
                tool_calls: vec![call()],
                usage: TokenUsage::default(),
            },
        };
        assert_eq!(
            serde_json::to_value(&completed).unwrap()["type"],
            "completed"
        );
        for item in [delta, completed] {
            let json = serde_json::to_string(&item).unwrap();
            assert_eq!(
                serde_json::from_str::<ModelStreamItem>(&json).unwrap(),
                item
            );
        }
    }

    #[test]
    fn request_and_response_round_trip() {
        let submission_id = crate::SubmissionId::new();
        let thread_id = crate::ThreadId::new();
        let turn_id = crate::TurnId::new();
        let message_id = crate::MessageId::new();
        let call = ToolCall::new(
            crate::ToolCallId::from("provider/call-7"),
            "echo",
            json!({"input": "hello"}),
        );
        let request = ModelRequest {
            system_prompt: Some("sys".into()),
            events: vec![
                Event {
                    id: submission_id,
                    log_position: 1,
                    thread_id,
                    turn_id: Some(turn_id),
                    timestamp_ms: 100,
                    msg: EventMsg::UserMessage(UserMessageEvent {
                        name: Some("alice".into()),
                        content: "hi".into(),
                    }),
                },
                Event {
                    id: submission_id,
                    log_position: 2,
                    thread_id,
                    turn_id: Some(turn_id),
                    timestamp_ms: 101,
                    msg: EventMsg::AssistantMessage(AssistantMessageEvent {
                        message_id,
                        content: Some("calling".into()),
                        tool_calls: vec![call.clone()],
                    }),
                },
                Event {
                    id: submission_id,
                    log_position: 3,
                    thread_id,
                    turn_id: Some(turn_id),
                    timestamp_ms: 102,
                    msg: EventMsg::ToolCallEnd(ToolCallEndEvent {
                        result: ToolResult::completed(&call, "hello"),
                        duration_ms: Some(5),
                    }),
                },
                Event {
                    id: submission_id,
                    log_position: 4,
                    thread_id,
                    turn_id: Some(turn_id),
                    timestamp_ms: 103,
                    msg: EventMsg::SystemReminder(SystemReminderEvent {
                        reminder: SystemReminder::ToolLimitReached,
                    }),
                },
            ],
            tools: vec![],
            iteration: 2,
        };
        let value = serde_json::to_value(&request).unwrap();
        assert!(value.get("events").is_some());
        assert!(value.get("history").is_none());
        assert_eq!(value["events"].as_array().unwrap().len(), 4);
        assert_eq!(value["events"][0]["msg"]["type"], "user_message");
        assert_eq!(value["events"][1]["msg"]["type"], "assistant_message");
        assert_eq!(value["events"][2]["msg"]["type"], "tool_call_end");
        assert_eq!(value["events"][3]["msg"]["type"], "system_reminder");
        assert_eq!(
            value["events"][1]["msg"]["tool_calls"][0]["id"],
            "provider/call-7"
        );
        assert_eq!(
            value["events"][2]["msg"]["result"]["call_id"],
            "provider/call-7"
        );
        assert_eq!(value["events"][0]["id"], submission_id.to_string());
        assert_eq!(value["events"][0]["thread_id"], thread_id.to_string());
        assert_eq!(value["events"][0]["turn_id"], turn_id.to_string());
        assert_eq!(
            value["events"][1]["msg"]["message_id"],
            message_id.to_string()
        );
        assert_eq!(value["events"][2]["log_position"], 3);
        assert_eq!(value["events"][2]["timestamp_ms"], 102);
        assert_eq!(
            serde_json::from_value::<ModelRequest>(value).unwrap(),
            request,
        );

        let response = ModelResponse {
            content: Some("ok".into()),
            tool_calls: vec![ToolCall::new("c1".into(), "echo", serde_json::json!({}))],
            usage: TokenUsage {
                input_tokens: 1,
                output_tokens: 2,
                ..Default::default()
            },
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains(r#""total_tokens":3"#));
        assert_eq!(
            serde_json::from_str::<ModelResponse>(&json).unwrap(),
            response
        );
    }
}
