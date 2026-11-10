//! Backend-neutral model protocol: what a model client receives and returns.

use serde::{Deserialize, Serialize};

use crate::common::TokenUsage;
use crate::history::HistoryItem;
use crate::tool::{ToolCall, ToolSpec};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::SystemReminder;

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
