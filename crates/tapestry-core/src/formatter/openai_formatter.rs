use super::Formatter;

/// Uses the default event projection and genai message conversion.
#[derive(Debug, Default, Clone, Copy)]
pub struct OpenAiFormatter;

impl Formatter for OpenAiFormatter {}

#[cfg(test)]
mod tests {
    use super::*;
    use genai::chat::ChatMessage;
    use tapestry_protocol::{
        AssistantMessageEvent, ConversationItem, EnvironmentContext,
        EnvironmentContextUpdatedEvent, Event, EventMsg, MessageId, SubmissionId, SystemReminder,
        SystemReminderEvent, ThreadId, ToolCall, ToolCallEndEvent, ToolCallId, ToolCallStatus,
        ToolResult, TurnAbortReason, TurnAbortedEvent, TurnStartedEvent, UserMessageEvent,
    };

    fn event(
        msg: EventMsg,
        turn_id: Option<tapestry_protocol::TurnId>,
        log_position: u64,
    ) -> Event {
        Event {
            id: SubmissionId::new(),
            log_position,
            thread_id: ThreadId::new(),
            turn_id,
            timestamp_ms: 0,
            msg,
        }
    }

    fn message_signatures(messages: &[ChatMessage]) -> Vec<String> {
        messages
            .iter()
            .map(|message| {
                let mut parts = vec![format!("{:?}", message.role)];
                if let Some(text) = message.content.first_text() {
                    parts.push(format!("text:{text}"));
                }
                parts.extend(message.content.tool_calls().into_iter().map(|call| {
                    format!(
                        "call:{}:{}:{}",
                        call.call_id, call.fn_name, call.fn_arguments
                    )
                }));
                parts.extend(
                    message
                        .content
                        .tool_responses()
                        .into_iter()
                        .map(|response| {
                            format!("result:{}:{}", response.call_id, response.content)
                        }),
                );
                parts.join("|")
            })
            .collect()
    }

    #[test]
    fn formats_message_bearing_events() {
        let call = ToolCall::new(
            ToolCallId::from("provider/id"),
            "echo",
            serde_json::json!({"input": "hello"}),
        );
        let turn_id = tapestry_protocol::TurnId::new();
        let events = vec![
            event(EventMsg::TurnStarted(TurnStartedEvent {}), Some(turn_id), 1),
            event(
                EventMsg::UserMessage(UserMessageEvent {
                    name: Some("alice".into()),
                    content: "hello".into(),
                }),
                Some(turn_id),
                2,
            ),
            event(
                EventMsg::EnvironmentContextUpdated(EnvironmentContextUpdatedEvent {
                    context: EnvironmentContext::Unavailable,
                }),
                Some(turn_id),
                3,
            ),
            event(
                EventMsg::AssistantMessage(AssistantMessageEvent {
                    message_id: MessageId::new(),
                    content: Some("calling".into()),
                    tool_calls: vec![call.clone()],
                }),
                Some(turn_id),
                4,
            ),
            event(
                EventMsg::ToolCallEnd(ToolCallEndEvent {
                    result: ToolResult::completed(&call, "hello"),
                    duration_ms: None,
                }),
                Some(turn_id),
                5,
            ),
            event(
                EventMsg::SystemReminder(SystemReminderEvent {
                    reminder: SystemReminder::custom("be brief"),
                }),
                Some(turn_id),
                6,
            ),
            event(
                EventMsg::TurnAborted(TurnAbortedEvent {
                    reason: TurnAbortReason::Interrupted,
                    message: None,
                }),
                Some(turn_id),
                7,
            ),
        ];
        let formatter: &dyn Formatter = &OpenAiFormatter;
        let items = formatter.events_to_items(&events);
        assert_eq!(items.len(), 5);
        let formatted = formatter.format(&events);
        assert_eq!(formatted.len(), 5);
        assert_eq!(formatted[0].content.first_text(), Some("alice: hello"));
        assert_eq!(
            formatted[1].content.first_text(),
            Some(
                crate::formatter::prompt::render_environment_context(
                    &EnvironmentContext::Unavailable
                )
                .as_str()
            )
        );
        assert_eq!(formatted[2].content.first_text(), Some("calling"));
        assert_eq!(formatted[2].content.tool_calls()[0].call_id, "provider/id");
        assert_eq!(
            formatted[2].content.tool_calls()[0].fn_arguments,
            serde_json::json!({"input": "hello"})
        );
        assert_eq!(
            formatted[3].content.tool_responses()[0].call_id,
            "provider/id"
        );
        assert_eq!(formatted[3].content.tool_responses()[0].content, "hello");
        assert_eq!(formatted[4].content.first_text(), Some("be brief"));
        assert!(items
            .iter()
            .any(|item| matches!(item, ConversationItem::SystemReminder(_))));
        assert_eq!(
            message_signatures(&formatted),
            message_signatures(&formatter.history_items_to_messages(&items))
        );
    }

    #[test]
    fn history_items_convert_to_messages() {
        let formatter = OpenAiFormatter;
        let formatter: &dyn Formatter = &formatter;
        let messages = formatter.history_items_to_messages(&[
            ConversationItem::user(None, "hello"),
            ConversationItem::system_reminder(SystemReminder::custom("be brief")),
        ]);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content.first_text(), Some("hello"));
        assert_eq!(messages[1].content.first_text(), Some("be brief"));
    }

    #[test]
    fn formatter_defaults_can_be_overridden_independently() {
        struct CustomProjection;

        impl Formatter for CustomProjection {
            fn events_to_items(&self, _events: &[Event]) -> Vec<ConversationItem> {
                vec![ConversationItem::user(None, "custom")]
            }
        }

        let formatter = CustomProjection;
        let messages: Vec<ChatMessage> = formatter.format(&[]);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content.first_text(), Some("custom"));
    }

    #[test]
    fn format_repairs_unfinished_tool_calls_before_mapping_messages() {
        let formatter = OpenAiFormatter;
        let turn_id = tapestry_protocol::TurnId::new();
        let call = ToolCall::new(
            ToolCallId::from("unfinished"),
            "echo",
            serde_json::json!({}),
        );
        let events = vec![
            event(EventMsg::TurnStarted(TurnStartedEvent {}), Some(turn_id), 1),
            event(
                EventMsg::AssistantMessage(AssistantMessageEvent {
                    message_id: MessageId::new(),
                    content: None,
                    tool_calls: vec![call.clone()],
                }),
                Some(turn_id),
                2,
            ),
            event(
                EventMsg::TurnAborted(TurnAbortedEvent {
                    reason: TurnAbortReason::Interrupted,
                    message: None,
                }),
                Some(turn_id),
                3,
            ),
        ];

        let history = formatter.events_to_items(&events);
        assert!(history.iter().any(|item| matches!(
            item,
            ConversationItem::ToolResult(result)
                if result.call_id == call.id && result.status == ToolCallStatus::Failed
        )));
        let messages = formatter.format(&events);
        assert_eq!(messages.len(), 2);
        assert_eq!(
            messages[1].content.tool_responses()[0].call_id,
            "unfinished"
        );
        assert_eq!(
            messages[1].content.tool_responses()[0].content,
            "tool call was not completed"
        );
    }

    #[test]
    fn incomplete_tool_result_precedes_valid_tail_reminder() {
        let formatter = OpenAiFormatter;
        let turn_id = tapestry_protocol::TurnId::new();
        let call = ToolCall::new(
            ToolCallId::from("provider/opaque"),
            "echo",
            serde_json::json!({}),
        );
        let events = vec![
            event(EventMsg::TurnStarted(TurnStartedEvent {}), Some(turn_id), 1),
            event(
                EventMsg::AssistantMessage(AssistantMessageEvent {
                    message_id: MessageId::new(),
                    content: None,
                    tool_calls: vec![call],
                }),
                Some(turn_id),
                2,
            ),
            event(
                EventMsg::SystemReminder(SystemReminderEvent {
                    reminder: SystemReminder::custom("finish now"),
                }),
                Some(turn_id),
                3,
            ),
            event(
                EventMsg::TurnAborted(TurnAbortedEvent {
                    reason: TurnAbortReason::Interrupted,
                    message: None,
                }),
                Some(turn_id),
                4,
            ),
        ];

        let items = formatter.events_to_items(&events);
        assert!(matches!(
            &items[1],
            ConversationItem::ToolResult(result)
                if result.call_id.as_str() == "provider/opaque"
                    && result.status == ToolCallStatus::Failed
        ));
        assert!(matches!(
            &items[2],
            ConversationItem::SystemReminder(SystemReminder::Custom { text }) if text == "finish now"
        ));

        let messages = formatter.format(&events);
        assert_eq!(messages.len(), 3);
        assert_eq!(
            messages[1].content.tool_responses()[0].call_id,
            "provider/opaque"
        );
        assert_eq!(
            messages[1].content.tool_responses()[0].content,
            "tool call was not completed"
        );
        assert_eq!(messages[2].content.first_text(), Some("finish now"));
        assert_eq!(
            message_signatures(&messages),
            message_signatures(&formatter.history_items_to_messages(&items))
        );
    }
}
