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
    Warning(WarningEvent),
    Error(ErrorEvent),
    SessionConfigured(SessionConfiguredEvent),
    TurnStarted(TurnStartedEvent),
    AgentMessage(AgentMessageEvent),
    ToolCallBegin(ToolCallBeginEvent),
    ToolCallEnd(ToolCallEndEvent),
    ModelRequestStarted(ModelRequestStartedEvent),
    ModelRequestCompleted(ModelRequestCompletedEvent),
    TokenCount(TokenCountEvent),
    TurnComplete(TurnCompleteEvent),
    TurnAborted(TurnAbortedEvent),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WarningEvent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    pub message: String,
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
    /// Authoritative outcome; `success` is kept for compatibility.
    #[serde(default)]
    pub status: ToolCallStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    #[default]
    Completed,
    Failed,
    NotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    #[default]
    Completed,
    ExceedMaxIters,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnAbortReason {
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRequestStartedEvent {
    pub turn_id: TurnId,
    pub iteration: u32,
    pub allow_tools: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRequestCompletedEvent {
    pub turn_id: TurnId,
    pub iteration: u32,
    pub input_tokens: u32,
    pub output_tokens: u32,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_agent_message: Option<String>,
    #[serde(default)]
    pub finish_reason: FinishReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnAbortedEvent {
    pub turn_id: Option<TurnId>,
    pub reason: TurnAbortReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
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
            status: ToolCallStatus::Completed,
            duration_ms: Some(3),
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
            last_agent_message: Some("done".into()),
            finish_reason: FinishReason::ExceedMaxIters,
        });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("turn_complete"));
        assert!(json.contains(r#""finish_reason":"exceed_max_iters""#));
        assert_eq!(msg, serde_json::from_str::<EventMsg>(&json).unwrap());
    }

    fn roundtrip(msg: EventMsg) {
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(msg, serde_json::from_str::<EventMsg>(&json).unwrap());
    }

    #[test]
    fn new_events_roundtrip() {
        let turn_id = TurnId::new();
        roundtrip(EventMsg::ModelRequestStarted(ModelRequestStartedEvent {
            turn_id,
            iteration: 1,
            allow_tools: false,
        }));
        roundtrip(EventMsg::ModelRequestCompleted(
            ModelRequestCompletedEvent {
                turn_id,
                iteration: 1,
                input_tokens: 4,
                output_tokens: 5,
            },
        ));
        roundtrip(EventMsg::Warning(WarningEvent {
            turn_id: Some(turn_id),
            message: "ignored".into(),
        }));
        roundtrip(EventMsg::Warning(WarningEvent {
            turn_id: None,
            message: "x".into(),
        }));
        roundtrip(EventMsg::ToolCallEnd(ToolCallEndEvent {
            turn_id,
            tool_call_id: ToolCallId::new(),
            tool_name: "nope".into(),
            output: "missing".into(),
            success: false,
            status: ToolCallStatus::NotFound,
            duration_ms: None,
        }));
    }

    #[test]
    fn tool_status_wire_format() {
        assert_eq!(
            serde_json::to_string(&ToolCallStatus::NotFound).unwrap(),
            r#""not_found""#
        );
    }

    #[test]
    fn turn_aborted_uses_enum_reason() {
        let msg = EventMsg::TurnAborted(TurnAbortedEvent {
            turn_id: None,
            reason: TurnAbortReason::Interrupted,
            message: None,
        });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""reason":"interrupted""#));
        assert!(!json.contains("message"));
        roundtrip(msg);
    }

    #[test]
    fn missing_new_fields_deserialize_to_defaults() {
        let id = TurnId::new();
        let end = format!(
            r#"{{"type":"tool_call_end","turn_id":"{id}","tool_call_id":"{}","tool_name":"e","output":"o","success":true}}"#,
            ToolCallId::new()
        );
        match serde_json::from_str::<EventMsg>(&end).unwrap() {
            EventMsg::ToolCallEnd(e) => {
                assert_eq!(e.status, ToolCallStatus::Completed);
                assert_eq!(e.duration_ms, None);
            }
            other => panic!("unexpected {other:?}"),
        }
        let done = format!(r#"{{"type":"turn_complete","turn_id":"{id}"}}"#);
        match serde_json::from_str::<EventMsg>(&done).unwrap() {
            EventMsg::TurnComplete(e) => {
                assert_eq!(e.finish_reason, FinishReason::Completed);
                assert_eq!(e.last_agent_message, None);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn thread_settings_agent_name_optional() {
        let s = ThreadSettings::default();
        assert_eq!(serde_json::to_string(&s).unwrap(), "{}");
        let s: ThreadSettings = serde_json::from_str(r#"{"agent_name":"planner"}"#).unwrap();
        assert_eq!(s.agent_name.as_deref(), Some("planner"));
        assert!(s.model.is_none());
    }
}
