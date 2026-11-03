//! Conversation history kept by a thread session.

use serde_json::Value;
use tapestry_protocol::{ToolCallId, ToolCallStatus};

/// A tool invocation requested by the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub id: ToolCallId,
    pub name: String,
    pub arguments: Value,
}

impl ToolCall {
    pub fn new(name: impl Into<String>, arguments: Value) -> Self {
        Self {
            id: ToolCallId::new(),
            name: name.into(),
            arguments,
        }
    }
}

/// One entry of the conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryItem {
    /// Input from a user-side speaker; `name` identifies the speaker.
    User {
        name: Option<String>,
        content: String,
    },
    /// Output of the agent itself. Never carries a name.
    Assistant {
        content: Option<String>,
        tool_calls: Vec<ToolCall>,
    },
    ToolResult {
        call_id: ToolCallId,
        tool_name: String,
        output: String,
        status: ToolCallStatus,
    },
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

    pub fn tool_result(
        call_id: ToolCallId,
        tool_name: impl Into<String>,
        output: impl Into<String>,
        status: ToolCallStatus,
    ) -> Self {
        Self::ToolResult {
            call_id,
            tool_name: tool_name.into(),
            output: output.into(),
            status,
        }
    }
}

/// Render a user message for the model: `"{name}: {content}"`, or the bare content.
pub fn render_user_content(name: Option<&str>, content: &str) -> String {
    match name {
        Some(name) => format!("{name}: {content}"),
        None => content.to_string(),
    }
}

/// Append a `failed` result for every trailing tool call that has none, so the
/// history stays valid for later model requests.
pub fn fill_missing_tool_results(history: &mut Vec<HistoryItem>) {
    let Some(assistant_idx) = history
        .iter()
        .rposition(|item| matches!(item, HistoryItem::Assistant { .. }))
    else {
        return;
    };
    let HistoryItem::Assistant { tool_calls, .. } = &history[assistant_idx] else {
        return;
    };

    let missing: Vec<HistoryItem> = tool_calls
        .iter()
        .filter(|call| {
            !history[assistant_idx + 1..].iter().any(|item| {
                matches!(item, HistoryItem::ToolResult { call_id, .. } if *call_id == call.id)
            })
        })
        .map(|call| {
            HistoryItem::tool_result(
                call.id,
                call.name.clone(),
                "tool call was not completed",
                ToolCallStatus::Failed,
            )
        })
        .collect();
    history.extend(missing);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_keep_construction_order() {
        let call = ToolCall::new("echo", serde_json::json!({"input": "x"}));
        let history = [
            HistoryItem::user(Some("alice".into()), "hi"),
            HistoryItem::assistant(None, vec![call.clone()]),
            HistoryItem::tool_result(call.id, "echo", "x", ToolCallStatus::Completed),
            HistoryItem::assistant(Some("done".into()), vec![]),
        ];
        assert!(matches!(&history[0], HistoryItem::User { name: Some(n), .. } if n == "alice"));
        assert!(
            matches!(&history[1], HistoryItem::Assistant { tool_calls, .. } if tool_calls.len() == 1)
        );
        assert!(
            matches!(&history[2], HistoryItem::ToolResult { call_id, .. } if *call_id == call.id)
        );
        assert!(
            matches!(&history[3], HistoryItem::Assistant { content: Some(c), .. } if c == "done")
        );
    }

    #[test]
    fn tool_calls_get_distinct_ids() {
        let a = ToolCall::new("echo", Value::Null);
        let b = ToolCall::new("echo", Value::Null);
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn render_with_name() {
        assert_eq!(render_user_content(Some("alice"), "hi"), "alice: hi");
    }

    #[test]
    fn render_without_name() {
        assert_eq!(render_user_content(None, "hi"), "hi");
    }

    #[test]
    fn render_empty_content() {
        assert_eq!(render_user_content(None, ""), "");
        assert_eq!(render_user_content(Some("bob"), ""), "bob: ");
    }

    #[test]
    fn fill_missing_adds_failed_results_for_unanswered_calls() {
        let a = ToolCall::new("echo", Value::Null);
        let b = ToolCall::new("echo", Value::Null);
        let mut history = vec![
            HistoryItem::user(None, "go"),
            HistoryItem::assistant(None, vec![a.clone(), b.clone()]),
            HistoryItem::tool_result(a.id, "echo", "ok", ToolCallStatus::Completed),
        ];
        fill_missing_tool_results(&mut history);
        assert_eq!(history.len(), 4);
        assert!(matches!(
            &history[3],
            HistoryItem::ToolResult { call_id, status: ToolCallStatus::Failed, .. } if *call_id == b.id
        ));

        let before = history.clone();
        fill_missing_tool_results(&mut history);
        assert_eq!(history, before);
    }

    #[test]
    fn fill_missing_noop_without_assistant() {
        let mut history = vec![HistoryItem::user(None, "hi")];
        fill_missing_tool_results(&mut history);
        assert_eq!(history.len(), 1);
    }
}
