//! AG-UI 1.0 adapter for Tapestry event streams.
//!
//! AG-UI 1.0 event mapping:
//!
//! | Tapestry event | AG-UI event(s) |
//! |---|---|
//! | `TurnStarted`, `TurnComplete` | `RUN_STARTED`, `RUN_FINISHED` |
//! | `TurnAborted`, in-turn `Error` | `RUN_ERROR` |
//! | `ModelRequestStarted`, `ModelRequestCompleted` | `STEP_STARTED`, `STEP_FINISHED` |
//! | assistant deltas and final message | `TEXT_MESSAGE_*` |
//! | assistant tool calls | `TOOL_CALL_START`, `TOOL_CALL_ARGS`, `TOOL_CALL_END` |
//! | `ToolCallEnd` | `TOOL_CALL_RESULT` |
//! | other events | dropped unless selected for `CUSTOM` |
//!
//! This crate defines the outbound subset of the AG-UI 1.0 schema and does not
//! implement transport, inbound messages, snapshots, or state events.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tapestry_protocol::{
    Event, EventAdapter, EventFilter, EventMsg, MessageId, ToolCallEndEvent, TurnId,
};

/// AG-UI event subset used by this adapter (AG-UI 1.0).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AguiEvent {
    RunStarted {
        #[serde(rename = "threadId")]
        thread_id: String,
        #[serde(rename = "runId")]
        run_id: String,
        timestamp: u64,
    },
    RunFinished {
        #[serde(rename = "threadId")]
        thread_id: String,
        #[serde(rename = "runId")]
        run_id: String,
        timestamp: u64,
    },
    RunError {
        #[serde(rename = "threadId")]
        thread_id: String,
        #[serde(rename = "runId")]
        run_id: String,
        message: String,
        code: String,
        timestamp: u64,
    },
    StepStarted {
        #[serde(rename = "stepName")]
        step_name: String,
        timestamp: u64,
    },
    StepFinished {
        #[serde(rename = "stepName")]
        step_name: String,
        timestamp: u64,
    },
    TextMessageStart {
        #[serde(rename = "messageId")]
        message_id: String,
        role: String,
        timestamp: u64,
    },
    TextMessageContent {
        #[serde(rename = "messageId")]
        message_id: String,
        delta: String,
        timestamp: u64,
    },
    TextMessageEnd {
        #[serde(rename = "messageId")]
        message_id: String,
        timestamp: u64,
    },
    ToolCallStart {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(rename = "toolCallName")]
        tool_call_name: String,
        #[serde(rename = "parentMessageId")]
        parent_message_id: String,
        timestamp: u64,
    },
    ToolCallArgs {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        delta: String,
        timestamp: u64,
    },
    ToolCallEnd {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        timestamp: u64,
    },
    ToolCallResult {
        #[serde(rename = "messageId")]
        message_id: String,
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        content: String,
        timestamp: u64,
    },
    Custom {
        name: String,
        value: Value,
        timestamp: u64,
    },
}

/// Stateful adapter for one thread's event stream.
pub struct AguiAdapter {
    custom: EventFilter,
    open_text: Option<MessageId>,
    open_run: Option<TurnId>,
    thread_id: Option<String>,
    last_timestamp: u64,
}

impl Default for AguiAdapter {
    fn default() -> Self {
        Self::new(EventFilter::none())
    }
}

impl AguiAdapter {
    /// Create an adapter with the categories to expose as `CUSTOM`.
    pub fn new(custom: EventFilter) -> Self {
        Self {
            custom,
            open_text: None,
            open_run: None,
            thread_id: None,
            last_timestamp: 0,
        }
    }

    fn close_text(&mut self, timestamp: u64) -> Vec<AguiEvent> {
        self.open_text
            .take()
            .map(|id| {
                vec![AguiEvent::TextMessageEnd {
                    message_id: id.to_string(),
                    timestamp,
                }]
            })
            .unwrap_or_default()
    }

    fn open_text(&mut self, id: MessageId, timestamp: u64) -> Vec<AguiEvent> {
        if self.open_text == Some(id) {
            return Vec::new();
        }
        let mut output = self.close_text(timestamp);
        self.open_text = Some(id);
        output.push(AguiEvent::TextMessageStart {
            message_id: id.to_string(),
            role: "assistant".into(),
            timestamp,
        });
        output
    }

    fn text_content(&self, id: MessageId, delta: &str, timestamp: u64) -> Option<AguiEvent> {
        (!delta.is_empty()).then(|| AguiEvent::TextMessageContent {
            message_id: id.to_string(),
            delta: delta.to_string(),
            timestamp,
        })
    }

    fn tool_result(&self, result: &ToolCallEndEvent, timestamp: u64) -> AguiEvent {
        let id = result.result.call_id.to_string();
        AguiEvent::ToolCallResult {
            message_id: format!("{id}:result"),
            tool_call_id: id,
            content: result.result.output.clone(),
            timestamp,
        }
    }

    fn custom_event(&self, event: &Event) -> Option<AguiEvent> {
        if !self.custom.matches(event) {
            return None;
        }
        let value = serde_json::to_value(&event.msg)
            .expect("Tapestry event payloads are JSON-serializable");
        Some(AguiEvent::Custom {
            name: format!("tapestry.{}", event.msg.type_name()),
            value,
            timestamp: event.timestamp_ms,
        })
    }

    fn run_error(&mut self, event: &Event, message: String, code: String) -> Vec<AguiEvent> {
        let mut output = self.close_text(event.timestamp_ms);
        let run_id = event.turn_id.or(self.open_run);
        if let (Some(run_id), Some(thread_id)) = (run_id, self.thread_id.clone()) {
            output.push(AguiEvent::RunError {
                thread_id,
                run_id: run_id.to_string(),
                message,
                code,
                timestamp: event.timestamp_ms,
            });
        }
        self.open_run = None;
        output
    }
}

impl EventAdapter for AguiAdapter {
    type Output = AguiEvent;

    fn adapt(&mut self, event: &Event) -> Vec<Self::Output> {
        self.last_timestamp = event.timestamp_ms;
        self.thread_id = Some(event.thread_id.to_string());
        match &event.msg {
            EventMsg::TurnStarted(_) => {
                let Some(run_id) = event.turn_id else {
                    return Vec::new();
                };
                self.open_run = Some(run_id);
                vec![AguiEvent::RunStarted {
                    thread_id: event.thread_id.to_string(),
                    run_id: run_id.to_string(),
                    timestamp: event.timestamp_ms,
                }]
            }
            EventMsg::ModelRequestStarted(model) => vec![AguiEvent::StepStarted {
                step_name: format!("model_request:{}", model.iteration),
                timestamp: event.timestamp_ms,
            }],
            EventMsg::ModelRequestCompleted(model) => vec![AguiEvent::StepFinished {
                step_name: format!("model_request:{}", model.iteration),
                timestamp: event.timestamp_ms,
            }],
            EventMsg::AssistantMessageStart(_) => Vec::new(),
            EventMsg::AssistantMessageDelta(delta) => {
                if delta.delta.is_empty() {
                    return Vec::new();
                }
                let mut output = self.open_text(delta.message_id, event.timestamp_ms);
                if let Some(content) =
                    self.text_content(delta.message_id, &delta.delta, event.timestamp_ms)
                {
                    output.push(content);
                }
                output
            }
            EventMsg::AssistantMessage(message) => {
                let had_deltas = self.open_text == Some(message.message_id);
                let mut output = Vec::new();
                if let Some(content) = message.content.as_deref().filter(|s| !s.is_empty()) {
                    output.extend(self.open_text(message.message_id, event.timestamp_ms));
                    if !had_deltas {
                        output.push(AguiEvent::TextMessageContent {
                            message_id: message.message_id.to_string(),
                            delta: content.to_string(),
                            timestamp: event.timestamp_ms,
                        });
                    }
                    output.extend(self.close_text(event.timestamp_ms));
                } else {
                    output.extend(self.close_text(event.timestamp_ms));
                }
                for call in &message.tool_calls {
                    let id = call.id.to_string();
                    output.push(AguiEvent::ToolCallStart {
                        tool_call_id: id.clone(),
                        tool_call_name: call.name.clone(),
                        parent_message_id: message.message_id.to_string(),
                        timestamp: event.timestamp_ms,
                    });
                    output.push(AguiEvent::ToolCallArgs {
                        tool_call_id: id.clone(),
                        delta: serde_json::to_string(&call.arguments)
                            .expect("JSON values always serialize"),
                        timestamp: event.timestamp_ms,
                    });
                    output.push(AguiEvent::ToolCallEnd {
                        tool_call_id: id,
                        timestamp: event.timestamp_ms,
                    });
                }
                output
            }
            EventMsg::ToolCallEnd(result) => vec![self.tool_result(result, event.timestamp_ms)],
            EventMsg::TurnComplete(_) => {
                let mut output = self.close_text(event.timestamp_ms);
                if let Some(run_id) = self.open_run.take() {
                    output.push(AguiEvent::RunFinished {
                        thread_id: event.thread_id.to_string(),
                        run_id: run_id.to_string(),
                        timestamp: event.timestamp_ms,
                    });
                }
                output
            }
            EventMsg::TurnAborted(aborted) => self.run_error(
                event,
                aborted
                    .message
                    .clone()
                    .unwrap_or_else(|| "turn aborted".into()),
                "interrupted".into(),
            ),
            EventMsg::Error(error) if event.turn_id.is_some() || self.open_run.is_some() => {
                self.run_error(event, error.message.clone(), "error".into())
            }
            _ => self.custom_event(event).into_iter().collect(),
        }
    }

    fn finish(&mut self) -> Vec<Self::Output> {
        let mut output = self.close_text(self.last_timestamp);
        if let (Some(run_id), Some(thread_id)) = (self.open_run.take(), self.thread_id.clone()) {
            output.push(AguiEvent::RunError {
                thread_id,
                run_id: run_id.to_string(),
                message: "event stream ended before the run completed".into(),
                code: "stream_ended".into(),
                timestamp: self.last_timestamp,
            });
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tapestry_protocol::{
        AssistantMessageDeltaEvent, AssistantMessageEvent, AssistantMessageStartEvent,
        EnvironmentContext, EnvironmentContextUpdatedEvent, ErrorEvent, Event, FinishReason,
        MessageId, ModelRequestCompletedEvent, ModelRequestStartedEvent, SessionConfiguredEvent,
        SubmissionId, SystemReminder, SystemReminderEvent, ThreadId, TokenCountEvent, TokenUsage,
        ToolCall, ToolCallBeginEvent, ToolCallEndEvent, ToolCallId, ToolResult, TurnAbortReason,
        TurnAbortedEvent, TurnCompleteEvent, TurnId, TurnStartedEvent, UserMessageEvent,
        WarningEvent,
    };

    fn event(log_position: u64, turn_id: Option<TurnId>, msg: EventMsg) -> Event {
        Event {
            id: SubmissionId::new(),
            log_position,
            thread_id: ThreadId::new(),
            turn_id,
            timestamp_ms: log_position * 10,
            msg,
        }
    }

    fn id() -> TurnId {
        TurnId::new()
    }

    fn collect(adapter: &mut AguiAdapter, events: &[Event]) -> Vec<AguiEvent> {
        events.iter().flat_map(|e| adapter.adapt(e)).collect()
    }

    fn event_types(events: &[AguiEvent]) -> Vec<String> {
        events
            .iter()
            .map(|event| {
                serde_json::to_value(event).unwrap()["type"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn wire_format_uses_agui_names_camel_case_and_timestamp() {
        let value = serde_json::to_value(AguiEvent::RunStarted {
            thread_id: "thread".into(),
            run_id: "run".into(),
            timestamp: 123,
        })
        .unwrap();
        assert_eq!(
            value,
            json!({"type":"RUN_STARTED","threadId":"thread","runId":"run","timestamp":123})
        );
        assert_eq!(
            serde_json::from_value::<AguiEvent>(value).unwrap(),
            AguiEvent::RunStarted {
                thread_id: "thread".into(),
                run_id: "run".into(),
                timestamp: 123,
            }
        );
    }

    #[test]
    fn successful_run_has_run_and_step_boundaries() {
        let turn = id();
        let thread = ThreadId::new();
        let mut adapter = AguiAdapter::default();
        let mut start = event(1, Some(turn), EventMsg::TurnStarted(TurnStartedEvent {}));
        start.thread_id = thread;
        let events = [
            start,
            event(
                2,
                Some(turn),
                EventMsg::ModelRequestStarted(ModelRequestStartedEvent {
                    iteration: 0,
                    allow_tools: true,
                }),
            ),
            event(
                3,
                Some(turn),
                EventMsg::ModelRequestCompleted(ModelRequestCompletedEvent {
                    iteration: 0,
                    usage: TokenUsage::default(),
                }),
            ),
            event(
                4,
                Some(turn),
                EventMsg::TurnComplete(TurnCompleteEvent {
                    last_agent_message: None,
                    finish_reason: FinishReason::Completed,
                }),
            ),
        ];
        let output = collect(&mut adapter, &events);
        assert!(
            matches!(output.first(), Some(AguiEvent::RunStarted { thread_id: t, run_id, timestamp: 10 }) if t == &thread.to_string() && run_id == &turn.to_string())
        );
        assert!(
            matches!(output[1], AguiEvent::StepStarted { ref step_name, .. } if step_name == "model_request:0")
        );
        assert!(
            matches!(output[2], AguiEvent::StepFinished { ref step_name, .. } if step_name == "model_request:0")
        );
        assert!(matches!(output.last(), Some(AguiEvent::RunFinished { .. })));
    }

    #[test]
    fn streaming_and_non_streaming_text_are_emitted_once_and_closed() {
        let turn = id();
        let message = MessageId::new();
        let mut adapter = AguiAdapter::default();
        let output = collect(
            &mut adapter,
            &[
                event(
                    1,
                    Some(turn),
                    EventMsg::AssistantMessageStart(AssistantMessageStartEvent {
                        message_id: message,
                    }),
                ),
                event(
                    2,
                    Some(turn),
                    EventMsg::AssistantMessageDelta(AssistantMessageDeltaEvent {
                        message_id: message,
                        delta: "Hel".into(),
                    }),
                ),
                event(
                    3,
                    Some(turn),
                    EventMsg::AssistantMessageDelta(AssistantMessageDeltaEvent {
                        message_id: message,
                        delta: "lo".into(),
                    }),
                ),
                event(
                    4,
                    Some(turn),
                    EventMsg::AssistantMessage(AssistantMessageEvent {
                        message_id: message,
                        content: Some("Hello".into()),
                        tool_calls: vec![],
                    }),
                ),
            ],
        );
        assert_eq!(
            event_types(&output),
            [
                "TEXT_MESSAGE_START",
                "TEXT_MESSAGE_CONTENT",
                "TEXT_MESSAGE_CONTENT",
                "TEXT_MESSAGE_END"
            ]
        );
        assert!(output.iter().all(|e| match e {
            AguiEvent::TextMessageStart { message_id: id, .. }
            | AguiEvent::TextMessageContent { message_id: id, .. }
            | AguiEvent::TextMessageEnd { message_id: id, .. } => id == &message.to_string(),
            _ => false,
        }));

        let mut adapter = AguiAdapter::default();
        let output = collect(
            &mut adapter,
            &[
                event(
                    1,
                    Some(turn),
                    EventMsg::AssistantMessageStart(AssistantMessageStartEvent {
                        message_id: message,
                    }),
                ),
                event(
                    2,
                    Some(turn),
                    EventMsg::AssistantMessage(AssistantMessageEvent {
                        message_id: message,
                        content: Some("hi".into()),
                        tool_calls: vec![],
                    }),
                ),
            ],
        );
        assert_eq!(
            event_types(&output),
            [
                "TEXT_MESSAGE_START",
                "TEXT_MESSAGE_CONTENT",
                "TEXT_MESSAGE_END"
            ]
        );
    }

    #[test]
    fn empty_text_and_tool_only_message_do_not_emit_text_events() {
        let turn = id();
        let call = ToolCall::new(ToolCallId::from("call-1"), "echo", json!({"input":"x"}));
        let mut adapter = AguiAdapter::default();
        let output = adapter.adapt(&event(
            1,
            Some(turn),
            EventMsg::AssistantMessage(AssistantMessageEvent {
                message_id: MessageId::new(),
                content: None,
                tool_calls: vec![call],
            }),
        ));
        assert_eq!(output.len(), 3);
        assert!(output.iter().all(|e| matches!(
            e,
            AguiEvent::ToolCallStart { .. }
                | AguiEvent::ToolCallArgs { .. }
                | AguiEvent::ToolCallEnd { .. }
        )));
    }

    #[test]
    fn tool_result_and_custom_filter() {
        let turn = id();
        let call = ToolCall::new(ToolCallId::from("call-1"), "echo", json!({}));
        let end = ToolCallEndEvent {
            result: ToolResult::completed(&call, "done"),
            duration_ms: Some(5),
        };
        let mut adapter = AguiAdapter::default();
        assert!(adapter
            .adapt(&event(
                1,
                Some(turn),
                EventMsg::TokenCount(TokenCountEvent {
                    usage: TokenUsage::default()
                })
            ))
            .is_empty());
        let output = adapter.adapt(&event(2, Some(turn), EventMsg::ToolCallEnd(end)));
        assert!(
            matches!(&output[0], AguiEvent::ToolCallResult { tool_call_id, message_id, content, .. } if tool_call_id == "call-1" && message_id == "call-1:result" && content == "done")
        );

        let filter = EventFilter::none().with(tapestry_protocol::EventCategory::Usage);
        let mut adapter = AguiAdapter::new(filter);
        let output = adapter.adapt(&event(
            3,
            Some(turn),
            EventMsg::TokenCount(TokenCountEvent {
                usage: TokenUsage::default(),
            }),
        ));
        assert!(
            matches!(&output[0], AguiEvent::Custom { name, value, .. } if name == "tapestry.token_count" && value["type"] == "token_count")
        );
    }

    #[test]
    fn errors_and_interrupt_close_text_before_run_error() {
        let turn = id();
        let message = MessageId::new();
        let mut adapter = AguiAdapter::default();
        let output = collect(
            &mut adapter,
            &[
                event(1, Some(turn), EventMsg::TurnStarted(TurnStartedEvent {})),
                event(
                    2,
                    Some(turn),
                    EventMsg::AssistantMessageDelta(AssistantMessageDeltaEvent {
                        message_id: message,
                        delta: "partial".into(),
                    }),
                ),
                event(
                    3,
                    Some(turn),
                    EventMsg::TurnAborted(TurnAbortedEvent {
                        reason: TurnAbortReason::Interrupted,
                        message: Some("stop".into()),
                    }),
                ),
            ],
        );
        assert!(matches!(
            output[output.len() - 2],
            AguiEvent::TextMessageEnd { .. }
        ));
        assert!(
            matches!(output.last(), Some(AguiEvent::RunError { code, .. }) if code == "interrupted")
        );
    }

    #[test]
    fn finish_closes_open_text_then_fails_open_run() {
        let turn = id();
        let message = MessageId::new();
        let mut adapter = AguiAdapter::default();
        adapter.adapt(&event(
            1,
            Some(turn),
            EventMsg::TurnStarted(TurnStartedEvent {}),
        ));
        adapter.adapt(&event(
            2,
            Some(turn),
            EventMsg::AssistantMessageDelta(AssistantMessageDeltaEvent {
                message_id: message,
                delta: "partial".into(),
            }),
        ));
        assert!(
            matches!(adapter.finish().as_slice(), [AguiEvent::TextMessageEnd { .. }, AguiEvent::RunError { code, .. }] if code == "stream_ended")
        );
        assert!(adapter.finish().is_empty());
    }

    #[test]
    fn non_turn_error_is_custom_only_when_selected() {
        let mut adapter = AguiAdapter::default();
        assert!(adapter
            .adapt(&event(
                1,
                None,
                EventMsg::Error(ErrorEvent {
                    message: "bad".into()
                })
            ))
            .is_empty());
        let mut adapter = AguiAdapter::new(EventFilter::all());
        assert!(matches!(
            adapter
                .adapt(&event(
                    1,
                    None,
                    EventMsg::Warning(WarningEvent {
                        message: "warn".into()
                    })
                ))
                .as_slice(),
            [AguiEvent::Custom { .. }]
        ));
        let _ = (
            SessionConfiguredEvent {
                thread_id: ThreadId::new(),
                model: "m".into(),
            },
            UserMessageEvent {
                name: None,
                content: "x".into(),
            },
            EnvironmentContextUpdatedEvent {
                context: EnvironmentContext::Unavailable,
            },
            SystemReminderEvent {
                reminder: SystemReminder::ToolLimitReached,
            },
            ToolCallBeginEvent {
                tool_call_id: "x".into(),
                tool_name: "x".into(),
                arguments: json!({}),
            },
        );
    }
}
