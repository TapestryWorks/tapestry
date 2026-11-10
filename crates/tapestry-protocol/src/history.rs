//! Conversation history data types shared between clients and the core.

use serde::{Deserialize, Serialize};

use crate::tool::{ToolCall, ToolResult};

/// Complete snapshot of the date and time zone told to the model.
///
/// The last one in history is the baseline for deciding whether to send another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum EnvironmentContext {
    Available { date: String, timezone: String },
    Unavailable,
}

/// Transient instruction for a single model request; never persisted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reminder", rename_all = "snake_case")]
pub enum SystemReminder {
    /// The tool iteration limit was reached; answer without tools.
    ToolLimitReached,
    /// Caller-defined reminder text, sent to the model verbatim.
    Custom { text: String },
}

impl SystemReminder {
    pub fn custom(text: impl Into<String>) -> Self {
        Self::Custom { text: text.into() }
    }
}

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
}
