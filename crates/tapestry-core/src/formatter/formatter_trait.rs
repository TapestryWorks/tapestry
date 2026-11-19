//! Conversation history as a projection of the event log.
//!
//! History is never written directly: [`HistoryProjection::apply`] folds each
//! appended event, so a thread's live history and a fresh fold of its log are
//! always equal.

use tapestry_protocol::{Event, EventMsg, HistoryItem, ToolResult, TurnId};

use crate::formatter::prompt::{render_environment_context, render_system_reminder, render_user};
use genai::chat::{
    ChatMessage, ContentPart, MessageContent, ToolCall as GenaiToolCall, ToolResponse,
};

/// Convert one event into its projected history entry, if any.
pub fn event_to_history_item(event: &Event) -> Option<HistoryItem> {
    match &event.msg {
        EventMsg::UserMessage(message) => Some(HistoryItem::user(
            message.name.clone(),
            message.content.clone(),
        )),
        EventMsg::AssistantMessage(message) => Some(HistoryItem::assistant(
            message.content.clone(),
            message.tool_calls.clone(),
        )),
        EventMsg::ToolCallEnd(end) => Some(end.result.clone().into()),
        EventMsg::EnvironmentContextUpdated(update) => Some(update.context.clone().into()),
        EventMsg::SystemReminder(reminder) => {
            Some(HistoryItem::system_reminder(reminder.reminder.clone()))
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
pub struct HistoryProjection {
    items: Vec<HistoryItem>,
    open_turn: Option<TurnId>,
}

impl HistoryProjection {
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

    pub fn history(&self) -> &[HistoryItem] {
        &self.items
    }

    pub fn apply(&mut self, event: &Event) {
        match &event.msg {
            EventMsg::TurnStarted(_) => {
                self.close_turn();
                self.open_turn = event.turn_id;
            }
            EventMsg::TurnComplete(_) => self.open_turn = None,
            EventMsg::TurnAborted(_) => self.close_turn(),
            EventMsg::Error(_) if event.turn_id.is_some() => self.close_turn(),
            _ => {}
        }

        if let Some(item) = event_to_history_item(event) {
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
/// history stays valid for later model requests.
fn fill_missing_tool_results(history: &mut Vec<HistoryItem>) {
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
    if missing.is_empty() {
        return;
    }

    let insert_at = history[assistant_idx + 1..]
        .iter()
        .rposition(|item| {
            matches!(
                item,
                HistoryItem::User { .. }
                    | HistoryItem::Assistant { .. }
                    | HistoryItem::ToolResult(_)
            )
        })
        .map_or(assistant_idx + 1, |index| assistant_idx + 2 + index);
    history.splice(insert_at..insert_at, missing);
}

/// Formatter for ordered event streams and normalized model history.
///
/// The defaults fold events into the current conversation history and map
/// history entries into genai chat messages. The complete projected history
/// retains reminders; the default message view filters reminders older than
/// the last user, assistant, or tool-result item. Override either stage when a
/// formatter needs custom projection or provider-specific conversion.
pub trait Formatter: Send + Sync {
    /// Fold events into persistent history, repairing unfinished turns.
    fn events_to_history_items(&self, events: &[Event]) -> Vec<HistoryItem> {
        HistoryProjection::fold(events).history().to_vec()
    }

    /// Convert the model-facing view of history items into genai chat messages.
    ///
    /// Reminders before the last user, assistant, or tool result are retained
    /// in projection but omitted from this sent view.
    fn history_items_to_messages(&self, items: &[HistoryItem]) -> Vec<ChatMessage> {
        let last_conversation_item = items.iter().rposition(|item| {
            matches!(
                item,
                HistoryItem::User { .. }
                    | HistoryItem::Assistant { .. }
                    | HistoryItem::ToolResult(_)
            )
        });
        items
            .iter()
            .enumerate()
            .filter(|(index, item)| {
                !matches!(item, HistoryItem::SystemReminder(_))
                    || last_conversation_item.is_none_or(|last| *index > last)
            })
            .map(|(_, item)| history_item_to_message(item))
            .collect()
    }

    /// Convert an event stream into genai chat messages using the defaults.
    fn format(&self, events: &[Event]) -> Vec<ChatMessage> {
        self.history_items_to_messages(&self.events_to_history_items(events))
    }
}

pub(crate) fn history_item_to_message(item: &HistoryItem) -> ChatMessage {
    match item {
        HistoryItem::User { name, content } => {
            ChatMessage::user(render_user(name.as_deref(), content))
        }
        HistoryItem::EnvironmentContext(context) => {
            ChatMessage::user(render_environment_context(context))
        }
        HistoryItem::SystemReminder(reminder) => {
            ChatMessage::user(render_system_reminder(reminder))
        }
        HistoryItem::Assistant {
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
        HistoryItem::ToolResult(result) => ChatMessage::from(
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
        EnvironmentContext, EnvironmentContextUpdatedEvent, ErrorEvent, MessageId, SubmissionId,
        SystemReminder, SystemReminderEvent, ThreadId, ToolCall, ToolCallEndEvent, ToolCallId,
        ToolCallStatus, TurnAbortReason, TurnAbortedEvent, TurnCompleteEvent, TurnStartedEvent,
        UserMessageEvent,
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

        fn fold(&self) -> Vec<HistoryItem> {
            HistoryProjection::fold(&self.events).history().to_vec()
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

    fn failed_result_for(history: &[HistoryItem], call: &ToolCall) -> bool {
        history.iter().any(|i| {
            matches!(i, HistoryItem::ToolResult(r)
                if r.call_id == call.id && r.status == ToolCallStatus::Failed)
        })
    }

    #[test]
    fn event_to_history_item_filters_non_history_events() {
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
            event_to_history_item(&event(EventMsg::UserMessage(UserMessageEvent {
                name: Some("alice".into()),
                content: "hi".into(),
            }))),
            Some(HistoryItem::user(Some("alice".into()), "hi"))
        );
        assert_eq!(
            event_to_history_item(&event(EventMsg::SystemReminder(SystemReminderEvent {
                iteration: 0,
                reminder: SystemReminder::ToolLimitReached,
            }))),
            Some(HistoryItem::system_reminder(
                SystemReminder::ToolLimitReached
            ))
        );
        assert!(
            event_to_history_item(&event(EventMsg::AssistantMessageDelta(
                AssistantMessageDeltaEvent {
                    message_id: MessageId::new(),
                    delta: "x".into(),
                }
            )))
            .is_none()
        );
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
                HistoryItem::user(Some("alice".into()), "hi"),
                HistoryItem::assistant(None, vec![a.clone()]),
                ToolResult::completed(&a, "ok").into(),
                HistoryItem::assistant(Some("done".into()), vec![]),
            ]
        );
    }

    #[test]
    fn system_reminder_is_preserved_in_projection_and_streaming_delta_is_not() {
        let mut log = Log::new();
        log.start("hi")
            .push(EventMsg::SystemReminder(SystemReminderEvent {
                iteration: 0,
                reminder: SystemReminder::ToolLimitReached,
            }))
            .assistant(Some("ok"), &[])
            .complete();
        let history = log.fold();
        assert_eq!(
            history,
            vec![
                HistoryItem::user(None, "hi"),
                HistoryItem::system_reminder(SystemReminder::ToolLimitReached),
                HistoryItem::assistant(Some("ok".into()), vec![]),
            ]
        );
    }

    #[test]
    fn partial_streamed_text_of_failed_turn_is_not_projected() {
        let mut log = Log::new();
        log.start("go")
            .stream(MessageId::new(), &["Hel", "lo"])
            .push(EventMsg::Error(ErrorEvent {
                message: "down".into(),
            }));
        assert_eq!(log.fold(), vec![HistoryItem::user(None, "go")]);
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
                HistoryItem::user(None, "go"),
                HistoryItem::assistant(Some("abc!".into()), vec![]),
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
        let mut live = HistoryProjection::new();
        for event in &log.events {
            live.apply(event);
        }
        assert_eq!(live.history(), history.as_slice());
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
        let mut live = HistoryProjection::new();
        for event in &log.events {
            live.apply(event);
        }
        assert!(failed_result_for(live.history(), &a));
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
    fn model_message_view_filters_only_reminders_before_last_conversation_item() {
        let context = HistoryItem::EnvironmentContext(EnvironmentContext::Unavailable);
        let result = HistoryItem::ToolResult(ToolResult {
            call_id: ToolCallId::from("opaque/call"),
            tool_name: "echo".into(),
            output: "result".into(),
            status: ToolCallStatus::Completed,
        });
        let cases = [
            (
                "last assistant keeps every later reminder",
                vec![
                    HistoryItem::system_reminder(SystemReminder::custom("old")),
                    HistoryItem::user(None, "hello"),
                    HistoryItem::assistant(Some("answer".into()), vec![]),
                    HistoryItem::system_reminder(SystemReminder::custom("tail 1")),
                    HistoryItem::system_reminder(SystemReminder::custom("tail 2")),
                ],
                vec![
                    "hello".into(),
                    "answer".into(),
                    "tail 1".into(),
                    "tail 2".into(),
                ],
            ),
            (
                "tool result expires earlier reminders",
                vec![
                    HistoryItem::user(None, "hello"),
                    HistoryItem::system_reminder(SystemReminder::custom("old")),
                    result.clone(),
                    HistoryItem::system_reminder(SystemReminder::custom("tail")),
                ],
                vec!["hello".into(), "result".into(), "tail".into()],
            ),
            (
                "reminders without conversation items are retained",
                vec![
                    HistoryItem::system_reminder(SystemReminder::custom("first")),
                    context.clone(),
                    HistoryItem::system_reminder(SystemReminder::custom("second")),
                ],
                vec![
                    "first".into(),
                    render_environment_context(&EnvironmentContext::Unavailable),
                    "second".into(),
                ],
            ),
            ("empty history stays empty", vec![], vec![]),
            (
                "environment context is not a reminder boundary",
                vec![
                    HistoryItem::user(None, "hello"),
                    HistoryItem::system_reminder(SystemReminder::custom("first")),
                    context.clone(),
                    HistoryItem::system_reminder(SystemReminder::custom("second")),
                ],
                vec![
                    "hello".into(),
                    "first".into(),
                    render_environment_context(&EnvironmentContext::Unavailable),
                    "second".into(),
                ],
            ),
            (
                "a later user message expires reminders",
                vec![
                    HistoryItem::system_reminder(SystemReminder::custom("old")),
                    HistoryItem::user(None, "new"),
                ],
                vec!["new".into()],
            ),
            (
                "a later assistant message expires reminders",
                vec![
                    HistoryItem::user(None, "hello"),
                    HistoryItem::system_reminder(SystemReminder::custom("old")),
                    HistoryItem::assistant(Some("answer".into()), vec![]),
                ],
                vec!["hello".into(), "answer".into()],
            ),
        ];

        let formatter = OpenAiFormatter;
        for (name, input, expected) in cases {
            let before = input.clone();
            let messages = formatter.history_items_to_messages(&input);
            assert_eq!(
                messages.iter().map(message_text).collect::<Vec<_>>(),
                expected,
                "{name}"
            );
            assert_eq!(input, before, "{name} modified its input");
        }
    }

    #[test]
    fn fill_missing_is_idempotent_and_needs_an_assistant() {
        let (a, b) = (call(), call());
        let mut history = vec![
            HistoryItem::user(None, "go"),
            HistoryItem::assistant(None, vec![a.clone(), b.clone()]),
            ToolResult::completed(&a, "ok").into(),
        ];
        fill_missing_tool_results(&mut history);
        assert_eq!(history.len(), 4);
        let before = history.clone();
        fill_missing_tool_results(&mut history);
        assert_eq!(history, before);

        let mut only_user = vec![HistoryItem::user(None, "hi")];
        fill_missing_tool_results(&mut only_user);
        assert_eq!(only_user.len(), 1);
    }

    #[test]
    fn missing_tool_results_are_inserted_before_valid_tail_reminders() {
        let (completed, missing) = (call(), call());
        let mut history = vec![
            HistoryItem::user(None, "go"),
            HistoryItem::assistant(None, vec![completed.clone(), missing.clone()]),
            ToolResult::completed(&completed, "ok").into(),
            HistoryItem::system_reminder(SystemReminder::ToolLimitReached),
            HistoryItem::EnvironmentContext(EnvironmentContext::Unavailable),
            HistoryItem::system_reminder(SystemReminder::custom("finish")),
        ];

        fill_missing_tool_results(&mut history);

        assert_eq!(history.len(), 7);
        assert!(matches!(
            &history[3],
            HistoryItem::ToolResult(result)
                if result.call_id == missing.id && result.status == ToolCallStatus::Failed
        ));
        assert!(matches!(
            history[4],
            HistoryItem::SystemReminder(SystemReminder::ToolLimitReached)
        ));
        assert!(matches!(
            history[6],
            HistoryItem::SystemReminder(SystemReminder::Custom { .. })
        ));
        let before = history.clone();
        fill_missing_tool_results(&mut history);
        assert_eq!(history, before);
    }

    #[test]
    fn live_projection_does_not_repair_a_normal_active_tool_call() {
        let call = call();
        let mut log = Log::new();
        log.start("go").assistant(None, std::slice::from_ref(&call));
        let mut live = HistoryProjection::new();
        for event in &log.events {
            live.apply(event);
        }
        assert!(!live
            .history()
            .iter()
            .any(|item| matches!(item, HistoryItem::ToolResult(_))));
    }
}
