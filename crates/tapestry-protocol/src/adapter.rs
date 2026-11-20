//! Protocol-independent event classification, filtering, and adapter contract.

use serde::{Deserialize, Serialize};

use crate::event::{Event, EventMsg};

/// Broad category assigned to each event variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventCategory {
    Session,
    Lifecycle,
    Message,
    Tool,
    Model,
    Context,
    Usage,
    Diagnostic,
}

impl EventMsg {
    /// Return the intrinsic category of this event.
    pub fn category(&self) -> EventCategory {
        match self {
            Self::SessionConfigured(_) => EventCategory::Session,
            Self::TurnStarted(_) | Self::TurnComplete(_) | Self::TurnAborted(_) => {
                EventCategory::Lifecycle
            }
            Self::UserMessage(_)
            | Self::AssistantMessageStart(_)
            | Self::AssistantMessageDelta(_)
            | Self::AssistantMessage(_) => EventCategory::Message,
            Self::ToolCallBegin(_) | Self::ToolCallEnd(_) => EventCategory::Tool,
            Self::ModelRequestStarted(_) | Self::ModelRequestCompleted(_) => EventCategory::Model,
            Self::EnvironmentContextUpdated(_) | Self::SystemReminder(_) => EventCategory::Context,
            Self::TokenCount(_) => EventCategory::Usage,
            Self::Warning(_) | Self::Error(_) => EventCategory::Diagnostic,
        }
    }

    /// Serialized snake-case tag for this event.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Warning(_) => "warning",
            Self::Error(_) => "error",
            Self::SessionConfigured(_) => "session_configured",
            Self::TurnStarted(_) => "turn_started",
            Self::UserMessage(_) => "user_message",
            Self::EnvironmentContextUpdated(_) => "environment_context_updated",
            Self::SystemReminder(_) => "system_reminder",
            Self::AssistantMessageStart(_) => "assistant_message_start",
            Self::AssistantMessageDelta(_) => "assistant_message_delta",
            Self::AssistantMessage(_) => "assistant_message",
            Self::ToolCallBegin(_) => "tool_call_begin",
            Self::ToolCallEnd(_) => "tool_call_end",
            Self::ModelRequestStarted(_) => "model_request_started",
            Self::ModelRequestCompleted(_) => "model_request_completed",
            Self::TokenCount(_) => "token_count",
            Self::TurnComplete(_) => "turn_complete",
            Self::TurnAborted(_) => "turn_aborted",
        }
    }
}

/// Set of event categories accepted by a consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventFilter(u8);

impl EventFilter {
    const fn bit(category: EventCategory) -> u8 {
        1 << (category as u8)
    }

    /// Match every category.
    pub const fn all() -> Self {
        Self(u8::MAX)
    }

    /// Match no categories.
    pub const fn none() -> Self {
        Self(0)
    }

    /// Include one additional category.
    pub const fn with(mut self, category: EventCategory) -> Self {
        self.0 |= Self::bit(category);
        self
    }

    /// Return whether this filter includes an event.
    pub fn matches(&self, event: &Event) -> bool {
        self.0 & Self::bit(event.msg.category()) != 0
    }
}

/// Stateful, synchronous conversion of one Tapestry event into zero or more
/// events in a consumer protocol.
pub trait EventAdapter {
    type Output;

    fn adapt(&mut self, event: &Event) -> Vec<Self::Output>;

    /// Flush any protocol-specific terminal output at the end of the stream.
    fn finish(&mut self) -> Vec<Self::Output> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::common::{
        EnvironmentContext, MessageId, SubmissionId, SystemReminder, ThreadId, TurnId,
    };
    use crate::event::*;
    use crate::tool::{ToolCall, ToolResult};

    fn event(msg: EventMsg) -> Event {
        Event {
            id: SubmissionId::new(),
            log_position: 1,
            thread_id: ThreadId::new(),
            turn_id: Some(TurnId::new()),
            timestamp_ms: 123,
            msg,
        }
    }

    fn all_messages() -> Vec<(EventMsg, EventCategory)> {
        vec![
            (
                EventMsg::SessionConfigured(SessionConfiguredEvent {
                    thread_id: ThreadId::new(),
                    model: "m".into(),
                }),
                EventCategory::Session,
            ),
            (
                EventMsg::TurnStarted(TurnStartedEvent {}),
                EventCategory::Lifecycle,
            ),
            (
                EventMsg::TurnComplete(TurnCompleteEvent {
                    last_agent_message: None,
                    finish_reason: Default::default(),
                }),
                EventCategory::Lifecycle,
            ),
            (
                EventMsg::TurnAborted(TurnAbortedEvent {
                    reason: TurnAbortReason::Interrupted,
                    message: None,
                }),
                EventCategory::Lifecycle,
            ),
            (
                EventMsg::UserMessage(UserMessageEvent {
                    name: None,
                    content: "x".into(),
                }),
                EventCategory::Message,
            ),
            (
                EventMsg::AssistantMessageStart(AssistantMessageStartEvent {
                    message_id: MessageId::new(),
                }),
                EventCategory::Message,
            ),
            (
                EventMsg::AssistantMessageDelta(AssistantMessageDeltaEvent {
                    message_id: MessageId::new(),
                    delta: "x".into(),
                }),
                EventCategory::Message,
            ),
            (
                EventMsg::AssistantMessage(AssistantMessageEvent {
                    message_id: MessageId::new(),
                    content: Some("x".into()),
                    tool_calls: vec![],
                }),
                EventCategory::Message,
            ),
            (
                EventMsg::ToolCallBegin(ToolCallBeginEvent {
                    tool_call_id: "c".into(),
                    tool_name: "t".into(),
                    arguments: json!({}),
                }),
                EventCategory::Tool,
            ),
            (
                EventMsg::ToolCallEnd(ToolCallEndEvent {
                    result: ToolResult::completed(&ToolCall::new("c".into(), "t", json!({})), "ok"),
                    duration_ms: None,
                }),
                EventCategory::Tool,
            ),
            (
                EventMsg::ModelRequestStarted(ModelRequestStartedEvent {
                    iteration: 0,
                    allow_tools: true,
                }),
                EventCategory::Model,
            ),
            (
                EventMsg::ModelRequestCompleted(ModelRequestCompletedEvent {
                    iteration: 0,
                    usage: Default::default(),
                }),
                EventCategory::Model,
            ),
            (
                EventMsg::EnvironmentContextUpdated(EnvironmentContextUpdatedEvent {
                    context: EnvironmentContext::Unavailable,
                }),
                EventCategory::Context,
            ),
            (
                EventMsg::SystemReminder(SystemReminderEvent {
                    iteration: 0,
                    reminder: SystemReminder::ToolLimitReached,
                }),
                EventCategory::Context,
            ),
            (
                EventMsg::TokenCount(TokenCountEvent {
                    usage: Default::default(),
                }),
                EventCategory::Usage,
            ),
            (
                EventMsg::Warning(WarningEvent {
                    message: "x".into(),
                }),
                EventCategory::Diagnostic,
            ),
            (
                EventMsg::Error(ErrorEvent {
                    message: "x".into(),
                }),
                EventCategory::Diagnostic,
            ),
        ]
    }

    #[test]
    fn every_event_has_exactly_its_declared_category_and_tag() {
        for (msg, category) in all_messages() {
            assert_eq!(msg.category(), category);
            assert_eq!(
                serde_json::to_value(category).unwrap(),
                json!(format!("{category:?}").to_lowercase())
            );
            let category_json = serde_json::to_string(&category).unwrap();
            assert_eq!(
                serde_json::from_str::<EventCategory>(&category_json).unwrap(),
                category
            );
            let tag = serde_json::to_value(&msg).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_string();
            assert_eq!(msg.type_name(), tag);
        }
    }

    #[test]
    fn filters_match_selected_categories_and_boundaries() {
        let filter = EventFilter::none()
            .with(EventCategory::Message)
            .with(EventCategory::Lifecycle);
        let started = event(EventMsg::TurnStarted(TurnStartedEvent {}));
        let assistant = event(EventMsg::AssistantMessageStart(
            AssistantMessageStartEvent {
                message_id: MessageId::new(),
            },
        ));
        let tool = event(EventMsg::ToolCallBegin(ToolCallBeginEvent {
            tool_call_id: "c".into(),
            tool_name: "t".into(),
            arguments: json!({}),
        }));
        let usage = event(EventMsg::TokenCount(TokenCountEvent {
            usage: Default::default(),
        }));
        assert!(filter.matches(&started));
        assert!(filter.matches(&assistant));
        assert!(!filter.matches(&tool));
        assert!(!filter.matches(&usage));
        assert!(EventFilter::all().matches(&usage));
        assert!(!EventFilter::none().matches(&started));
    }

    struct TwoOutputs;
    impl EventAdapter for TwoOutputs {
        type Output = &'static str;
        fn adapt(&mut self, _event: &Event) -> Vec<Self::Output> {
            vec!["first", "second"]
        }
    }

    #[test]
    fn adapters_can_emit_multiple_items_and_default_finish_is_empty() {
        let mut adapter = TwoOutputs;
        assert_eq!(
            adapter.adapt(&event(EventMsg::TurnStarted(TurnStartedEvent {}))),
            ["first", "second"]
        );
        assert!(adapter.finish().is_empty());
    }
}
