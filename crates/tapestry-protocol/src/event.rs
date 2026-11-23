use serde::{Deserialize, Serialize};

use crate::common::{
    EnvironmentContext, FinishReason, MessageId, SubmissionId, SystemReminder, ThreadId,
    TokenUsage, ToolCallId, TurnId,
};
use crate::tool::{ToolCall, ToolResult};

/// One entry of a thread's event log — the single source of truth for what the
/// agent did. Consumers pick the events they need.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// Submission id this event correlates with.
    pub id: SubmissionId,
    /// Position in the thread's log, starting at 1 and assigned by the store.
    pub log_position: u64,
    pub thread_id: ThreadId,
    /// Turn this event belongs to; absent for events outside any turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    /// Unix epoch milliseconds.
    pub timestamp_ms: u64,
    pub msg: EventMsg,
}

/// Agent events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventMsg {
    Warning(WarningEvent),
    Error(ErrorEvent),
    SessionConfigured(SessionConfiguredEvent),
    TurnStarted(TurnStartedEvent),
    UserMessage(UserMessageEvent),
    EnvironmentContextUpdated(EnvironmentContextUpdatedEvent),
    SystemReminder(SystemReminderEvent),
    AssistantMessageStart(AssistantMessageStartEvent),
    AssistantMessageDelta(AssistantMessageDeltaEvent),
    AssistantMessage(AssistantMessageEvent),
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnStartedEvent {}

/// The user input that started a turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserMessageEvent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub content: String,
}

/// A complete environment context snapshot added to the history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentContextUpdatedEvent {
    pub context: EnvironmentContext,
}

/// A reminder recorded as an event and consumed by the next completed model request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemReminderEvent {
    pub reminder: SystemReminder,
}

/// Opens an assistant message; deltas and the final message share its id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistantMessageStartEvent {
    pub message_id: MessageId,
}

/// One streamed text fragment of an assistant message, in arrival order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistantMessageDeltaEvent {
    pub message_id: MessageId,
    pub delta: String,
}

/// A complete assistant entry: optional text plus the tool calls it requested.
///
/// The text comes from the model's final response, not from the deltas.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistantMessageEvent {
    pub message_id: MessageId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallBeginEvent {
    pub tool_call_id: ToolCallId,
    pub tool_name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallEndEvent {
    pub result: ToolResult,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnAbortReason {
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRequestStartedEvent {
    pub iteration: u32,
    pub allow_tools: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRequestCompletedEvent {
    pub iteration: u32,
    pub usage: TokenUsage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCountEvent {
    /// Summed over every model call of the turn.
    pub usage: TokenUsage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnCompleteEvent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_agent_message: Option<String>,
    #[serde(default)]
    pub finish_reason: FinishReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnAbortedEvent {
    pub reason: TurnAbortReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::tool::ToolCallStatus;

    fn call() -> ToolCall {
        ToolCall::new(ToolCallId::from("call_1"), "echo", json!({}))
    }

    fn result(status: ToolCallStatus) -> ToolResult {
        let call = call();
        ToolResult {
            call_id: call.id,
            tool_name: call.name,
            output: "hi".into(),
            status,
        }
    }

    fn envelope(turn_id: Option<TurnId>, msg: EventMsg) -> Event {
        Event {
            id: SubmissionId::new(),
            log_position: 7,
            thread_id: ThreadId::new(),
            turn_id,
            timestamp_ms: 1_700_000_000_000,
            msg,
        }
    }

    fn roundtrip(msg: EventMsg) {
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(msg, serde_json::from_str::<EventMsg>(&json).unwrap());
    }

    fn tag(msg: &EventMsg) -> String {
        serde_json::to_value(msg).unwrap()["type"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn turn_event_envelope_carries_turn_id() {
        let turn_id = TurnId::new();
        let event = envelope(
            Some(turn_id),
            EventMsg::ToolCallBegin(ToolCallBeginEvent {
                tool_call_id: ToolCallId::from("call_1"),
                tool_name: "echo".into(),
                arguments: json!({"text": "hi"}),
            }),
        );
        let value = serde_json::to_value(&event).unwrap();
        for key in [
            "id",
            "log_position",
            "thread_id",
            "turn_id",
            "timestamp_ms",
            "msg",
        ] {
            assert!(value.get(key).is_some(), "missing {key}");
        }
        assert!(value.get("seq").is_none());
        assert_eq!(value["turn_id"], json!(turn_id.to_string()));
        assert_eq!(value["log_position"], 7);
        assert!(value["msg"].get("turn_id").is_none());
        assert_eq!(serde_json::from_value::<Event>(value).unwrap(), event);
    }

    #[test]
    fn non_turn_event_omits_turn_id() {
        let event = envelope(
            None,
            EventMsg::SessionConfigured(SessionConfiguredEvent {
                thread_id: ThreadId::new(),
                model: "mock".into(),
            }),
        );
        let value = serde_json::to_value(&event).unwrap();
        assert!(value.get("turn_id").is_none());
        assert_eq!(serde_json::from_value::<Event>(value).unwrap(), event);
    }

    #[test]
    fn new_history_events_roundtrip_with_snake_case_tags() {
        let cases = [
            (
                EventMsg::UserMessage(UserMessageEvent {
                    name: Some("alice".into()),
                    content: "hi".into(),
                }),
                "user_message",
            ),
            (
                EventMsg::AssistantMessage(AssistantMessageEvent {
                    message_id: MessageId::new(),
                    content: None,
                    tool_calls: vec![call()],
                }),
                "assistant_message",
            ),
            (
                EventMsg::AssistantMessageStart(AssistantMessageStartEvent {
                    message_id: MessageId::new(),
                }),
                "assistant_message_start",
            ),
            (
                EventMsg::AssistantMessageDelta(AssistantMessageDeltaEvent {
                    message_id: MessageId::new(),
                    delta: "Hel".into(),
                }),
                "assistant_message_delta",
            ),
            (
                EventMsg::EnvironmentContextUpdated(EnvironmentContextUpdatedEvent {
                    context: EnvironmentContext::Available {
                        date: "2026-11-05".into(),
                        timezone: "UTC".into(),
                    },
                }),
                "environment_context_updated",
            ),
            (
                EventMsg::SystemReminder(SystemReminderEvent {
                    reminder: SystemReminder::ToolLimitReached,
                }),
                "system_reminder",
            ),
            (EventMsg::TurnStarted(TurnStartedEvent {}), "turn_started"),
        ];
        for (msg, expected) in cases {
            assert_eq!(tag(&msg), expected);
            roundtrip(msg);
        }
    }

    #[test]
    fn assistant_message_wire_format_omits_empty_fields() {
        let message_id = MessageId::new();
        let msg = EventMsg::AssistantMessage(AssistantMessageEvent {
            message_id,
            content: Some("done".into()),
            tool_calls: vec![],
        });
        assert_eq!(
            serde_json::to_value(&msg).unwrap(),
            json!({"type": "assistant_message", "message_id": message_id, "content": "done"})
        );
        let delta = EventMsg::AssistantMessageDelta(AssistantMessageDeltaEvent {
            message_id,
            delta: "do".into(),
        });
        assert_eq!(
            serde_json::to_value(&delta).unwrap(),
            json!({"type": "assistant_message_delta", "message_id": message_id, "delta": "do"})
        );
        let user = EventMsg::UserMessage(UserMessageEvent {
            name: None,
            content: "hi".into(),
        });
        assert_eq!(
            serde_json::to_value(&user).unwrap(),
            json!({"type": "user_message", "content": "hi"})
        );
    }

    #[test]
    fn system_reminder_event_wire_format() {
        let msg = EventMsg::SystemReminder(SystemReminderEvent {
            reminder: SystemReminder::custom("be brief"),
        });
        assert_eq!(
            serde_json::to_value(&msg).unwrap(),
            json!({
                "type": "system_reminder",
                "reminder": {"reminder": "custom", "text": "be brief"}
            })
        );
    }

    #[test]
    fn system_reminder_event_ignores_legacy_iteration() {
        let msg: EventMsg = serde_json::from_value(json!({
            "type": "system_reminder",
            "iteration": 1,
            "reminder": {"reminder": "tool_limit_reached"}
        }))
        .unwrap();
        assert_eq!(
            msg,
            EventMsg::SystemReminder(SystemReminderEvent {
                reminder: SystemReminder::ToolLimitReached,
            })
        );
    }

    #[test]
    fn token_count_event_fields() {
        let msg = EventMsg::TokenCount(TokenCountEvent {
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 20,
                ..Default::default()
            },
        });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""total_tokens":30"#));
        roundtrip(msg);
    }

    #[test]
    fn turn_complete_event_roundtrip() {
        let msg = EventMsg::TurnComplete(TurnCompleteEvent {
            last_agent_message: Some("done".into()),
            finish_reason: FinishReason::ExceedMaxIters,
        });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("turn_complete"));
        assert!(json.contains(r#""finish_reason":"exceed_max_iters""#));
        roundtrip(msg);
    }

    #[test]
    fn model_and_tool_events_roundtrip() {
        roundtrip(EventMsg::ModelRequestStarted(ModelRequestStartedEvent {
            iteration: 1,
            allow_tools: false,
        }));
        roundtrip(EventMsg::ModelRequestCompleted(
            ModelRequestCompletedEvent {
                iteration: 1,
                usage: TokenUsage {
                    input_tokens: 4,
                    output_tokens: 5,
                    cache_read_tokens: 1,
                    cache_write_tokens: 0,
                },
            },
        ));
        roundtrip(EventMsg::Warning(WarningEvent {
            message: "ignored".into(),
        }));
        roundtrip(EventMsg::ToolCallEnd(ToolCallEndEvent {
            result: result(ToolCallStatus::NotFound),
            duration_ms: None,
        }));
    }

    #[test]
    fn tool_call_end_carries_result_without_success() {
        let msg = EventMsg::ToolCallEnd(ToolCallEndEvent {
            result: result(ToolCallStatus::Failed),
            duration_ms: Some(1),
        });
        let value = serde_json::to_value(&msg).unwrap();
        assert!(value.get("success").is_none());
        assert_eq!(value["result"]["status"], "failed");
        assert_eq!(value["result"]["call_id"], "call_1");
        roundtrip(msg);
    }

    #[test]
    fn turn_aborted_uses_enum_reason() {
        let msg = EventMsg::TurnAborted(TurnAbortedEvent {
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
        match serde_json::from_str::<EventMsg>(r#"{"type":"turn_complete"}"#).unwrap() {
            EventMsg::TurnComplete(e) => {
                assert_eq!(e.finish_reason, FinishReason::Completed);
                assert_eq!(e.last_agent_message, None);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
