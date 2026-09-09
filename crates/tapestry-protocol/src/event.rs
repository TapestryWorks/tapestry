use serde::{Deserialize, Serialize};

use crate::ids::{SubmissionId, ThreadId, ToolCallId, TurnId};

/// Event Queue entry — a response from the agent to the client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// Submission id this event correlates with.
    pub id: SubmissionId,
    pub msg: EventMsg,
}

impl Event {
    pub fn new(id: SubmissionId, msg: EventMsg) -> Self {
        Self { id, msg }
    }
}

/// Agent response events.
///
/// Mirrors the core Codex `EventMsg` variants needed for turn execution and
/// streaming UI updates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventMsg {
    Error(ErrorEvent),
    SessionConfigured(SessionConfiguredEvent),
    TurnStarted(TurnStartedEvent),
    AgentMessage(AgentMessageEvent),
    ToolCallBegin(ToolCallBeginEvent),
    ToolCallEnd(ToolCallEndEvent),
    TokenCount(TokenCountEvent),
    TurnComplete(TurnCompleteEvent),
    TurnAborted(TurnAbortedEvent),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorEvent {
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionConfiguredEvent {
    pub thread_id: ThreadId,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnStartedEvent {
    pub turn_id: TurnId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMessageEvent {
    pub turn_id: TurnId,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallBeginEvent {
    pub turn_id: TurnId,
    pub tool_call_id: ToolCallId,
    pub tool_name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallEndEvent {
    pub turn_id: TurnId,
    pub tool_call_id: ToolCallId,
    pub tool_name: String,
    pub output: String,
    pub success: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCountEvent {
    pub turn_id: TurnId,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnCompleteEvent {
    pub turn_id: TurnId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnAbortedEvent {
    pub turn_id: Option<TurnId>,
    pub reason: String,
}

/// Mutable thread/session settings applied via [`crate::submission::Op::ThreadSettings`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ThreadSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tool_iterations: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::SubmissionId;

    #[test]
    fn event_msg_agent_message_roundtrip() {
        let event = Event::new(
            SubmissionId::new(),
            EventMsg::AgentMessage(AgentMessageEvent {
                turn_id: TurnId::new(),
                content: "Hello from agent".into(),
            }),
        );
        let json = serde_json::to_string(&event).unwrap();
        let decoded: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(event, decoded);
    }

    #[test]
    fn tool_call_events_have_distinct_types() {
        let turn_id = TurnId::new();
        let tool_call_id = ToolCallId::new();

        let begin = EventMsg::ToolCallBegin(ToolCallBeginEvent {
            turn_id,
            tool_call_id,
            tool_name: "echo".into(),
            arguments: serde_json::json!({"text": "hi"}),
        });
        let end = EventMsg::ToolCallEnd(ToolCallEndEvent {
            turn_id,
            tool_call_id,
            tool_name: "echo".into(),
            output: "hi".into(),
            success: true,
        });

        let begin_json = serde_json::to_string(&begin).unwrap();
        let end_json = serde_json::to_string(&end).unwrap();

        assert!(begin_json.contains("tool_call_begin"));
        assert!(end_json.contains("tool_call_end"));
    }

    #[test]
    fn token_count_event_fields() {
        let msg = EventMsg::TokenCount(TokenCountEvent {
            turn_id: TurnId::new(),
            input_tokens: 10,
            output_tokens: 20,
            total_tokens: 30,
        });
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: EventMsg = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn turn_complete_event_roundtrip() {
        let msg = EventMsg::TurnComplete(TurnCompleteEvent {
            turn_id: TurnId::new(),
        });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("turn_complete"));
    }
}
