//! Conversation history kept by a thread session.

use tapestry_protocol::{HistoryItem, ToolResult};

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
            !history[assistant_idx + 1..].iter().any(
                |item| matches!(item, HistoryItem::ToolResult(result) if result.call_id == call.id),
            )
        })
        .map(|call| HistoryItem::from(ToolResult::failed(call, "tool call was not completed")))
        .collect();
    history.extend(missing);
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use tapestry_protocol::{ToolCall, ToolCallId, ToolCallStatus};

    use super::*;

    #[test]
    fn items_keep_construction_order() {
        let call = ToolCall::new(
            ToolCallId::generate(),
            "echo",
            serde_json::json!({"input": "x"}),
        );
        let history = [
            HistoryItem::user(Some("alice".into()), "hi"),
            HistoryItem::assistant(None, vec![call.clone()]),
            ToolResult::completed(&call, "x").into(),
            HistoryItem::assistant(Some("done".into()), vec![]),
        ];
        assert!(matches!(&history[0], HistoryItem::User { name: Some(n), .. } if n == "alice"));
        assert!(
            matches!(&history[1], HistoryItem::Assistant { tool_calls, .. } if tool_calls.len() == 1)
        );
        assert!(matches!(&history[2], HistoryItem::ToolResult(r) if r.call_id == call.id));
        assert!(
            matches!(&history[3], HistoryItem::Assistant { content: Some(c), .. } if c == "done")
        );
    }

    #[test]
    fn fill_missing_adds_failed_results_for_unanswered_calls() {
        let a = ToolCall::new(ToolCallId::generate(), "echo", Value::Null);
        let b = ToolCall::new(ToolCallId::generate(), "echo", Value::Null);
        let mut history = vec![
            HistoryItem::user(None, "go"),
            HistoryItem::assistant(None, vec![a.clone(), b.clone()]),
            ToolResult::completed(&a, "ok").into(),
        ];
        fill_missing_tool_results(&mut history);
        assert_eq!(history.len(), 4);
        assert!(matches!(
            &history[3],
            HistoryItem::ToolResult(r) if r.call_id == b.id && r.status == ToolCallStatus::Failed
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
