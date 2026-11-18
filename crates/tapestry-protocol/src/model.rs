//! Backend-neutral model protocol: what a model client receives and returns.

use serde::{Deserialize, Serialize};

use crate::common::{EnvironmentContext, SystemReminder, TokenUsage};
use crate::tool::{ToolCall, ToolResult, ToolSpec};

/// One entry of the conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HistoryItem {
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
    /// Only present in request copies of history, never in the stored history.
    SystemReminder(SystemReminder),
}

impl HistoryItem {
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

impl From<ToolResult> for HistoryItem {
    fn from(result: ToolResult) -> Self {
        Self::ToolResult(result)
    }
}

impl From<EnvironmentContext> for HistoryItem {
    fn from(context: EnvironmentContext) -> Self {
        Self::EnvironmentContext(context)
    }
}

/// Conversation state passed to the model client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRequest {
    pub system_prompt: Option<String>,
    pub history: Vec<HistoryItem>,
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
    use serde_json::json;

    use super::*;

    fn call() -> ToolCall {
        ToolCall::new("call_1".into(), "echo", json!({"input": "x"}))
    }

    #[test]
    fn items_round_trip() {
        let call = call();
        let items = vec![
            HistoryItem::user(Some("alice".into()), "hi"),
            HistoryItem::assistant(None, vec![call.clone()]),
            ToolResult::completed(&call, "x").into(),
            HistoryItem::assistant(Some("done".into()), vec![]),
            EnvironmentContext::Available {
                date: "2026-11-05".into(),
                timezone: "UTC".into(),
            }
            .into(),
            EnvironmentContext::Unavailable.into(),
            HistoryItem::system_reminder(SystemReminder::ToolLimitReached),
            HistoryItem::system_reminder(SystemReminder::custom("be brief")),
        ];
        let text = serde_json::to_string(&items).unwrap();
        let back: Vec<HistoryItem> = serde_json::from_str(&text).unwrap();
        assert_eq!(back, items);
    }

    #[test]
    fn wire_format_is_tagged_and_omits_empty_fields() {
        let user = serde_json::to_value(HistoryItem::user(None, "hi")).unwrap();
        assert_eq!(user, json!({"type": "user", "content": "hi"}));

        let assistant =
            serde_json::to_value(HistoryItem::assistant(Some("ok".into()), vec![])).unwrap();
        assert_eq!(assistant, json!({"type": "assistant", "content": "ok"}));

        let parsed: HistoryItem = serde_json::from_value(json!({"type": "assistant"})).unwrap();
        assert_eq!(parsed, HistoryItem::assistant(None, vec![]));
    }

    #[test]
    fn tool_result_wire_format_is_flat() {
        let value =
            serde_json::to_value(HistoryItem::from(ToolResult::not_found(&call()))).unwrap();
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
        let available = HistoryItem::from(EnvironmentContext::Available {
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
            serde_json::to_value(HistoryItem::from(EnvironmentContext::Unavailable)).unwrap(),
            json!({"type": "environment_context", "state": "unavailable"})
        );
    }

    #[test]
    fn system_reminder_wire_format() {
        assert_eq!(
            serde_json::to_value(HistoryItem::system_reminder(
                SystemReminder::ToolLimitReached
            ))
            .unwrap(),
            json!({"type": "system_reminder", "reminder": "tool_limit_reached"})
        );
    }

    #[test]
    fn custom_system_reminder_wire_format() {
        assert_eq!(
            serde_json::to_value(HistoryItem::system_reminder(SystemReminder::custom(
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
        let request = ModelRequest {
            system_prompt: Some("sys".into()),
            history: vec![
                HistoryItem::user(None, "hi"),
                HistoryItem::system_reminder(SystemReminder::ToolLimitReached),
            ],
            tools: vec![],
            iteration: 2,
        };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(
            serde_json::from_str::<ModelRequest>(&json).unwrap(),
            request
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
