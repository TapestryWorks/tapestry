//! Conversation as a projection of the event log.
//!
//! The conversation is never written directly: [`ConversationProjection::apply`]
//! folds each appended event, so a thread's live projection and a fresh fold of
//! its log are always equal.

use tapestry_protocol::{ConversationItem, Event, EventMsg, SystemReminder, ToolResult, TurnId};

use crate::formatter::prompt::{render_environment_context, render_system_reminder, render_user};
use genai::chat::{
    ChatMessage, ContentPart, MessageContent, ToolCall as GenaiToolCall, ToolResponse,
};

/// Convert one event into its projected conversation item, if any.
pub fn event_to_item(event: &Event) -> Option<ConversationItem> {
    match &event.msg {
        EventMsg::UserMessage(message) => Some(ConversationItem::user(
            message.name.clone(),
            message.content.clone(),
        )),
        EventMsg::AssistantMessage(message) => Some(ConversationItem::assistant(
            message.content.clone(),
            message.tool_calls.clone(),
        )),
        EventMsg::ToolCallEnd(end) => Some(end.result.clone().into()),
        EventMsg::EnvironmentContextUpdated(update) => Some(update.context.clone().into()),
        EventMsg::SystemReminder(reminder) => {
            Some(ConversationItem::system_reminder(reminder.reminder.clone()))
        }
        EventMsg::Warning(_)
        | EventMsg::Error(_)
        | EventMsg::SessionConfigured(_)
        | EventMsg::TurnStarted(_)
        | EventMsg::AssistantMessageStart(_)
        | EventMsg::AssistantMessageDelta(_)
        | EventMsg::ToolCallBegin(_)
        | EventMsg::ModelRequestStarted(_)
        | EventMsg::ModelRequestCompleted(_)
        | EventMsg::TokenCount(_)
        | EventMsg::TurnComplete(_)
        | EventMsg::TurnAborted(_) => None,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConversationProjection {
    items: Vec<ConversationItem>,
    /// Reminders no completed model request has consumed yet.
    reminders: Vec<SystemReminder>,
    open_turn: Option<TurnId>,
}

impl ConversationProjection {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold a whole log. A turn the log leaves open (truncated) is repaired.
    pub fn fold<'a>(events: impl IntoIterator<Item = &'a Event>) -> Self {
        let mut projection = Self::new();
        for event in events {
            projection.apply(event);
        }
        projection.close_turn();
        projection
    }

    /// The conversation so far; valid reminders always come last.
    pub fn messages(&self) -> Vec<ConversationItem> {
        self.items
            .iter()
            .cloned()
            .chain(
                self.reminders
                    .iter()
                    .cloned()
                    .map(ConversationItem::system_reminder),
            )
            .collect()
    }

    pub fn apply(&mut self, event: &Event) {
        match &event.msg {
            EventMsg::TurnStarted(_) => {
                self.close_turn();
                self.reminders.clear();
                self.open_turn = event.turn_id;
            }
            EventMsg::TurnComplete(_) => self.open_turn = None,
            EventMsg::TurnAborted(_) => self.close_turn(),
            EventMsg::Error(_) if event.turn_id.is_some() => self.close_turn(),
            EventMsg::ModelRequestCompleted(_) => self.reminders.clear(),
            EventMsg::SystemReminder(reminder) => {
                self.reminders.push(reminder.reminder.clone());
                return;
            }
            _ => {}
        }

        if let Some(item) = event_to_item(event) {
            self.items.push(item);
        }
    }

    /// End the open turn, if any, without `TurnComplete`: every tool call
    /// left without a result gets a `failed` one.
    fn close_turn(&mut self) {
        if self.open_turn.take().is_some() {
            fill_missing_tool_results(&mut self.items);
        }
    }
}

/// Append a `failed` result for every trailing tool call that has none, so the
/// conversation stays valid for later model requests.
fn fill_missing_tool_results(items: &mut Vec<ConversationItem>) {
    let Some(assistant_idx) = items
        .iter()
        .rposition(|item| matches!(item, ConversationItem::Assistant { .. }))
    else {
        return;
    };
    let ConversationItem::Assistant { tool_calls, .. } = &items[assistant_idx] else {
        return;
    };

    let missing: Vec<ConversationItem> = tool_calls
        .iter()
        .filter(|call| {
            !items[assistant_idx + 1..].iter().any(
                |item| matches!(item, ConversationItem::ToolResult(result) if result.call_id == call.id),
            )
        })
        .map(|call| ConversationItem::from(ToolResult::failed(call, "tool call was not completed")))
        .collect();
    items.extend(missing);
}

/// Formatter for ordered event streams and normalized conversation items.
///
/// The defaults fold events into the current conversation (valid reminders
/// last) and map every item into a genai chat message. Override either stage
/// when a formatter needs custom projection or provider-specific conversion.
pub trait Formatter: Send + Sync {
    /// Fold events into conversation items, repairing unfinished turns.
    fn events_to_items(&self, events: &[Event]) -> Vec<ConversationItem> {
        ConversationProjection::fold(events).messages()
    }

    /// Convert conversation items into genai chat messages, in order.
    fn history_items_to_messages(&self, items: &[ConversationItem]) -> Vec<ChatMessage> {
        items.iter().map(history_item_to_message).collect()
    }

    /// Convert an event stream into genai chat messages using the defaults.
    fn format(&self, events: &[Event]) -> Vec<ChatMessage> {
        self.history_items_to_messages(&self.events_to_items(events))
    }
}

pub(crate) fn history_item_to_message(item: &ConversationItem) -> ChatMessage {
    match item {
        ConversationItem::User { name, content } => {
            ChatMessage::user(render_user(name.as_deref(), content))
        }
        ConversationItem::EnvironmentContext(context) => {
            ChatMessage::user(render_environment_context(context))
        }
        ConversationItem::SystemReminder(reminder) => {
            ChatMessage::user(render_system_reminder(reminder))
        }
        ConversationItem::Assistant {
            content,
            tool_calls,
        } => {
            if tool_calls.is_empty() {
                return ChatMessage::assistant(content.clone().unwrap_or_default());
            }
            let mut parts = Vec::new();
            if let Some(text) = content.as_ref().filter(|text| !text.is_empty()) {
                parts.push(ContentPart::Text(text.clone()));
            }
            parts.extend(tool_calls.iter().map(|call| {
                ContentPart::ToolCall(GenaiToolCall {
                    call_id: call.id.as_str().to_string(),
                    fn_name: call.name.clone(),
                    fn_arguments: call.arguments.clone(),
                    thought_signatures: None,
                })
            }));
            ChatMessage::assistant(MessageContent::from_parts(parts))
        }
        ConversationItem::ToolResult(result) => ChatMessage::from(
            ToolResponse::new(result.call_id.as_str().to_string(), result.output.clone())
                .with_fn_name(result.tool_name.clone()),
        ),
    }
}

#[cfg(test)]
mod tests {
    use crate::formatter::prompt::render_environment_context;
    use crate::formatter::OpenAiFormatter;
    use serde_json::Value;
    use tapestry_protocol::{
        AssistantMessageDeltaEvent, AssistantMessageEvent, AssistantMessageStartEvent,
        EnvironmentContext, EnvironmentContextUpdatedEvent, ErrorEvent, MessageId,
        ModelRequestCompletedEvent, ModelRequestStartedEvent, SubmissionId, SystemReminder,
        SystemReminderEvent, ThreadId, ToolCall, ToolCallEndEvent, ToolCallId, ToolCallStatus,
        TurnAbortReason, TurnAbortedEvent, TurnCompleteEvent, TurnStartedEvent, UserMessageEvent,
    };

    use super::*;

    struct Log {
        turn: Option<TurnId>,
        events: Vec<Event>,
    }

    impl Log {
        fn new() -> Self {
            Self {
                turn: None,
                events: Vec::new(),
            }
        }

        fn push(&mut self, msg: EventMsg) -> &mut Self {
            self.events.push(Event {
                id: SubmissionId::new(),
                log_position: self.events.len() as u64 + 1,
                thread_id: ThreadId::new(),
                turn_id: self.turn,
                timestamp_ms: 0,
                msg,
            });
            self
        }

        fn start(&mut self, content: &str) -> &mut Self {
            self.turn = Some(TurnId::new());
            self.push(EventMsg::TurnStarted(TurnStartedEvent {}))
                .push(EventMsg::UserMessage(UserMessageEvent {
                    name: None,
                    content: content.into(),
                }))
        }

        fn assistant(&mut self, content: Option<&str>, calls: &[ToolCall]) -> &mut Self {
            self.push(EventMsg::AssistantMessage(AssistantMessageEvent {
                message_id: MessageId::new(),
                content: content.map(str::to_string),
                tool_calls: calls.to_vec(),
            }))
        }

        fn stream(&mut self, id: MessageId, deltas: &[&str]) -> &mut Self {
            self.push(EventMsg::AssistantMessageStart(
                AssistantMessageStartEvent { message_id: id },
            ));
            for delta in deltas {
                self.push(EventMsg::AssistantMessageDelta(
                    AssistantMessageDeltaEvent {
                        message_id: id,
                        delta: delta.to_string(),
                    },
                ));
            }
            self
        }

        fn tool_end(&mut self, call: &ToolCall) -> &mut Self {
            self.push(EventMsg::ToolCallEnd(ToolCallEndEvent {
                result: ToolResult::completed(call, "ok"),
                duration_ms: None,
            }))
        }

        fn complete(&mut self) -> &mut Self {
            self.push(EventMsg::TurnComplete(TurnCompleteEvent {
                last_agent_message: None,
                finish_reason: Default::default(),
            }))
        }

        fn fold(&self) -> Vec<ConversationItem> {
            ConversationProjection::fold(&self.events).messages()
        }
    }

    fn call() -> ToolCall {
        ToolCall::new(ToolCallId::generate(), "echo", Value::Null)
    }

    fn message_text(message: &ChatMessage) -> String {
        message
            .content
            .first_text()
            .map(str::to_string)
            .or_else(|| {
                message
                    .content
                    .tool_responses()
                    .first()
                    .map(|response| response.content.clone())
            })
            .expect("message has text")
    }

    fn failed_result_for(history: &[ConversationItem], call: &ToolCall) -> bool {
        history.iter().any(|i| {
            matches!(i, ConversationItem::ToolResult(r)
                if r.call_id == call.id && r.status == ToolCallStatus::Failed)
        })
    }

    #[test]
    fn event_to_item_filters_non_history_events() {
        let turn = TurnId::new();
        let event = |msg: EventMsg| Event {
            id: SubmissionId::new(),
            log_position: 1,
            thread_id: ThreadId::new(),
            turn_id: Some(turn),
            timestamp_ms: 0,
            msg,
        };
        assert_eq!(
            event_to_item(&event(EventMsg::UserMessage(UserMessageEvent {
                name: Some("alice".into()),
                content: "hi".into(),
            }))),
            Some(ConversationItem::user(Some("alice".into()), "hi"))
        );
        assert_eq!(
            event_to_item(&event(EventMsg::SystemReminder(SystemReminderEvent {
                reminder: SystemReminder::ToolLimitReached,
            }))),
            Some(ConversationItem::system_reminder(
                SystemReminder::ToolLimitReached
            ))
        );
        assert!(event_to_item(&event(EventMsg::AssistantMessageDelta(
            AssistantMessageDeltaEvent {
                message_id: MessageId::new(),
                delta: "x".into(),
            }
        )))
        .is_none());
    }

    #[test]
    fn events_map_to_history_items() {
        let a = call();
        let context = EnvironmentContext::Available {
            date: "2026-11-05".into(),
            timezone: "UTC".into(),
        };
        let mut log = Log::new();
        log.turn = Some(TurnId::new());
        log.push(EventMsg::TurnStarted(TurnStartedEvent {}))
            .push(EventMsg::EnvironmentContextUpdated(
                EnvironmentContextUpdatedEvent {
                    context: context.clone(),
                },
            ))
            .push(EventMsg::UserMessage(UserMessageEvent {
                name: Some("alice".into()),
                content: "hi".into(),
            }))
            .assistant(None, std::slice::from_ref(&a))
            .tool_end(&a)
            .assistant(Some("done"), &[])
            .complete();
        assert_eq!(
            log.fold(),
            vec![
                context.into(),
                ConversationItem::user(Some("alice".into()), "hi"),
                ConversationItem::assistant(None, vec![a.clone()]),
                ToolResult::completed(&a, "ok").into(),
                ConversationItem::assistant(Some("done".into()), vec![]),
            ]
        );
    }

    fn reminder(text: &str) -> EventMsg {
        EventMsg::SystemReminder(SystemReminderEvent {
            reminder: SystemReminder::custom(text),
        })
    }

    fn reminder_item(text: &str) -> ConversationItem {
        ConversationItem::system_reminder(SystemReminder::custom(text))
    }

    fn request_started() -> EventMsg {
        EventMsg::ModelRequestStarted(ModelRequestStartedEvent {
            iteration: 0,
            allow_tools: true,
        })
    }

    fn request_completed() -> EventMsg {
        EventMsg::ModelRequestCompleted(ModelRequestCompletedEvent {
            iteration: 0,
            usage: Default::default(),
        })
    }

    /// Incremental `apply` must agree with `fold` for every prefix of `log`.
    fn assert_live_equals_fold(log: &Log) {
        let mut live = ConversationProjection::new();
        for (index, event) in log.events.iter().enumerate() {
            live.apply(event);
            let mut closed = live.clone();
            closed.close_turn();
            assert_eq!(
                closed,
                ConversationProjection::fold(&log.events[..=index]),
                "prefix {index}"
            );
        }
    }

    #[test]
    fn unconsumed_reminder_is_projected_last() {
        let mut log = Log::new();
        log.start("hi").push(request_started()).push(reminder("r"));
        assert_eq!(
            log.fold(),
            vec![ConversationItem::user(None, "hi"), reminder_item("r")]
        );
    }

    #[test]
    fn reminder_before_tool_result_is_still_delivered_after_it() {
        let a = call();
        let mut log = Log::new();
        log.start("go")
            .push(request_started())
            .assistant(None, std::slice::from_ref(&a))
            .push(reminder("r"))
            .tool_end(&a);
        assert_eq!(
            log.fold(),
            vec![
                ConversationItem::user(None, "go"),
                ConversationItem::assistant(None, vec![a.clone()]),
                ToolResult::completed(&a, "ok").into(),
                reminder_item("r"),
            ]
        );
    }

    #[test]
    fn completed_request_consumes_reminders_even_without_output() {
        let mut log = Log::new();
        log.start("hi")
            .push(request_started())
            .push(reminder("r"))
            .push(request_completed());
        assert_eq!(log.fold(), vec![ConversationItem::user(None, "hi")]);
    }

    #[test]
    fn reminders_keep_order_and_are_consumed_together() {
        let mut log = Log::new();
        log.start("hi")
            .push(request_started())
            .push(reminder("a"))
            .push(reminder("b"));
        assert_eq!(
            log.fold(),
            vec![
                ConversationItem::user(None, "hi"),
                reminder_item("a"),
                reminder_item("b")
            ]
        );
        log.push(request_completed());
        assert_eq!(log.fold(), vec![ConversationItem::user(None, "hi")]);
    }

    #[test]
    fn reminder_recorded_after_a_completed_request_survives_until_the_next_one() {
        let mut log = Log::new();
        log.start("hi")
            .push(request_started())
            .push(request_completed())
            .push(reminder("late"));
        assert_eq!(
            log.fold(),
            vec![ConversationItem::user(None, "hi"), reminder_item("late")]
        );
    }

    #[test]
    fn new_turn_discards_unconsumed_reminders() {
        let mut log = Log::new();
        log.start("one")
            .push(request_started())
            .push(reminder("stale"))
            .push(EventMsg::Error(ErrorEvent {
                message: "down".into(),
            }));
        log.start("two");
        assert_eq!(
            log.fold(),
            vec![
                ConversationItem::user(None, "one"),
                ConversationItem::user(None, "two")
            ]
        );
    }

    #[test]
    fn reminders_follow_synthesized_failed_results() {
        let a = call();
        let mut log = Log::new();
        log.start("go")
            .assistant(None, std::slice::from_ref(&a))
            .push(reminder("r"));
        let items = log.fold();
        assert_eq!(items.len(), 4);
        assert!(failed_result_for(&items, &a));
        assert_eq!(items[3], reminder_item("r"));
    }

    #[test]
    fn incremental_apply_matches_fold_with_reminders() {
        let a = call();
        let mut log = Log::new();
        log.start("go")
            .push(request_started())
            .push(reminder("x"))
            .assistant(None, std::slice::from_ref(&a))
            .push(reminder("y"))
            .tool_end(&a)
            .push(request_completed())
            .push(reminder("z"))
            .push(request_started())
            .assistant(Some("done"), &[])
            .push(request_completed())
            .complete();
        assert_live_equals_fold(&log);
        log.start("next")
            .push(request_started())
            .assistant(None, std::slice::from_ref(&a))
            .push(reminder("w"));
        assert_live_equals_fold(&log);
    }

    #[test]
    fn partial_streamed_text_of_failed_turn_is_not_projected() {
        let mut log = Log::new();
        log.start("go")
            .stream(MessageId::new(), &["Hel", "lo"])
            .push(EventMsg::Error(ErrorEvent {
                message: "down".into(),
            }));
        assert_eq!(log.fold(), vec![ConversationItem::user(None, "go")]);
    }

    #[test]
    fn streamed_message_yields_one_entry_with_final_text() {
        let id = MessageId::new();
        let mut log = Log::new();
        log.start("go").stream(id, &["a", "b", "c"]);
        log.push(EventMsg::AssistantMessage(AssistantMessageEvent {
            message_id: id,
            content: Some("abc!".into()),
            tool_calls: vec![],
        }))
        .complete();
        assert_eq!(
            log.fold(),
            vec![
                ConversationItem::user(None, "go"),
                ConversationItem::assistant(Some("abc!".into()), vec![]),
            ]
        );
    }

    #[test]
    fn error_mid_turn_repairs_unanswered_calls() {
        let (a, b) = (call(), call());
        let mut log = Log::new();
        log.start("go")
            .assistant(None, &[a.clone(), b.clone()])
            .tool_end(&a)
            .push(EventMsg::Error(ErrorEvent {
                message: "down".into(),
            }));
        let history = log.fold();
        assert_eq!(history.len(), 4);
        assert!(failed_result_for(&history, &b));
        assert!(!failed_result_for(&history, &a));

        // Applying incrementally gives the same result as folding.
        let mut live = ConversationProjection::new();
        for event in &log.events {
            live.apply(event);
        }
        assert_eq!(live.messages(), history);
    }

    #[test]
    fn truncated_log_repairs_open_turn() {
        let a = call();
        let mut log = Log::new();
        log.start("go").assistant(None, std::slice::from_ref(&a));
        assert!(failed_result_for(&log.fold(), &a));
    }

    #[test]
    fn next_turn_start_repairs_previous_open_turn() {
        let a = call();
        let mut log = Log::new();
        log.start("go").assistant(None, std::slice::from_ref(&a));
        log.start("again");
        let mut live = ConversationProjection::new();
        for event in &log.events {
            live.apply(event);
        }
        assert!(failed_result_for(&live.messages(), &a));
    }

    #[test]
    fn turn_aborted_repairs() {
        let a = call();
        let mut log = Log::new();
        log.start("go")
            .assistant(None, std::slice::from_ref(&a))
            .push(EventMsg::TurnAborted(TurnAbortedEvent {
                reason: TurnAbortReason::Interrupted,
                message: None,
            }));
        assert!(failed_result_for(&log.fold(), &a));
    }

    #[test]
    fn completed_turn_and_errors_outside_turns_add_nothing() {
        let a = call();
        let mut log = Log::new();
        log.start("go")
            .assistant(None, std::slice::from_ref(&a))
            .tool_end(&a)
            .assistant(Some("x"), &[])
            .complete();
        log.turn = None;
        log.push(EventMsg::Error(ErrorEvent {
            message: "not configured".into(),
        }));
        assert_eq!(log.fold().len(), 4);
    }

    #[test]
    fn default_message_view_maps_every_item_in_order() {
        let result = ConversationItem::ToolResult(ToolResult {
            call_id: ToolCallId::from("opaque/call"),
            tool_name: "echo".into(),
            output: "result".into(),
            status: ToolCallStatus::Completed,
        });
        let input = vec![
            ConversationItem::user(None, "hello"),
            result,
            ConversationItem::EnvironmentContext(EnvironmentContext::Unavailable),
            reminder_item("first"),
            reminder_item("second"),
        ];
        let before = input.clone();
        let messages = OpenAiFormatter.history_items_to_messages(&input);
        assert_eq!(
            messages.iter().map(message_text).collect::<Vec<_>>(),
            vec![
                "hello".to_string(),
                "result".to_string(),
                render_environment_context(&EnvironmentContext::Unavailable),
                "first".to_string(),
                "second".to_string(),
            ]
        );
        assert_eq!(input, before);
        assert!(OpenAiFormatter.history_items_to_messages(&[]).is_empty());
    }

    #[test]
    fn fill_missing_is_idempotent_and_needs_an_assistant() {
        let (a, b) = (call(), call());
        let mut history = vec![
            ConversationItem::user(None, "go"),
            ConversationItem::assistant(None, vec![a.clone(), b.clone()]),
            ToolResult::completed(&a, "ok").into(),
        ];
        fill_missing_tool_results(&mut history);
        assert_eq!(history.len(), 4);
        let before = history.clone();
        fill_missing_tool_results(&mut history);
        assert_eq!(history, before);

        let mut only_user = vec![ConversationItem::user(None, "hi")];
        fill_missing_tool_results(&mut only_user);
        assert_eq!(only_user.len(), 1);
    }

    #[test]
    fn live_projection_does_not_repair_a_normal_active_tool_call() {
        let call = call();
        let mut log = Log::new();
        log.start("go").assistant(None, std::slice::from_ref(&call));
        let mut live = ConversationProjection::new();
        for event in &log.events {
            live.apply(event);
        }
        assert!(!live
            .messages()
            .iter()
            .any(|item| matches!(item, ConversationItem::ToolResult(_))));
    }
}
