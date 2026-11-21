//! Command-line output adapter for Tapestry event streams.
//!
//! Only tool calls (`ToolCallBegin`), tool results (`ToolCallEnd`) and
//! assistant text (`AssistantMessage` with non-empty content) produce output;
//! every other event is dropped. Each output renders as one human-readable
//! entry through [`std::fmt::Display`]:
//!
//! | Tapestry event | Rendering |
//! |---|---|
//! | `ToolCallBegin` | `tool call: <name>(<arguments JSON>)` (`<name>()` for empty arguments) |
//! | `ToolCallEnd` | `tool result: <name>: <output>`, or `tool result: <name> (<status>): <output>` when not completed |
//! | `AssistantMessage` | `<content>` (no prefix) |
//! | `AssistantMessageDelta` (opt-in) | raw delta text |
//!
//! One adapter instance handles one thread.

use std::fmt;

use serde_json::Value;
use tapestry_protocol::{Event, EventAdapter, EventMsg, ToolCallStatus};

/// Options controlling which events reach command-line output.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CliAdapterOptions {
    /// Also emit `AssistantMessageDelta` text.
    pub include_deltas: bool,
}

/// One command-line output entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliOutput {
    ToolCall {
        tool_name: String,
        arguments: Value,
    },
    ToolResult {
        tool_name: String,
        status: ToolCallStatus,
        output: String,
    },
    Assistant {
        content: String,
    },
    AssistantDelta {
        delta: String,
    },
}

impl fmt::Display for CliOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ToolCall {
                tool_name,
                arguments,
            } => {
                let empty = match arguments {
                    Value::Null => true,
                    Value::Object(map) => map.is_empty(),
                    _ => false,
                };
                if empty {
                    write!(f, "tool call: {tool_name}()")
                } else {
                    write!(f, "tool call: {tool_name}({arguments})")
                }
            }
            Self::ToolResult {
                tool_name,
                status: ToolCallStatus::Completed,
                output,
            } => write!(f, "tool result: {tool_name}: {output}"),
            Self::ToolResult {
                tool_name,
                status,
                output,
            } => {
                let status = match status {
                    ToolCallStatus::Completed => "completed",
                    ToolCallStatus::Failed => "failed",
                    ToolCallStatus::NotFound => "not found",
                };
                write!(f, "tool result: {tool_name} ({status}): {output}")
            }
            Self::Assistant { content } => f.write_str(content),
            Self::AssistantDelta { delta } => f.write_str(delta),
        }
    }
}

/// Converts Tapestry events into command-line output entries.
#[derive(Debug, Clone, Default)]
pub struct CliEventAdapter {
    options: CliAdapterOptions,
}

impl CliEventAdapter {
    /// Create an adapter with the default options (deltas dropped).
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an adapter with custom options.
    pub fn with_options(options: CliAdapterOptions) -> Self {
        Self { options }
    }

    /// Options used by this adapter.
    pub fn options(&self) -> CliAdapterOptions {
        self.options
    }
}

impl EventAdapter for CliEventAdapter {
    type Output = CliOutput;

    fn adapt(&mut self, event: &Event) -> Vec<Self::Output> {
        let output = match &event.msg {
            EventMsg::ToolCallBegin(call) => Some(CliOutput::ToolCall {
                tool_name: call.tool_name.clone(),
                arguments: call.arguments.clone(),
            }),
            EventMsg::ToolCallEnd(end) => Some(CliOutput::ToolResult {
                tool_name: end.result.tool_name.clone(),
                status: end.result.status,
                output: end.result.output.clone(),
            }),
            // Tool-call-only messages are already shown by `ToolCallBegin`.
            EventMsg::AssistantMessage(message) => message
                .content
                .as_ref()
                .filter(|content| !content.trim().is_empty())
                .map(|content| CliOutput::Assistant {
                    content: content.clone(),
                }),
            EventMsg::AssistantMessageDelta(delta) if self.options.include_deltas => {
                Some(CliOutput::AssistantDelta {
                    delta: delta.delta.clone(),
                })
            }
            _ => None,
        };
        output.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tapestry_protocol::*;

    use super::*;

    fn events(msgs: Vec<EventMsg>) -> Vec<Event> {
        let id = SubmissionId::new();
        let thread_id = ThreadId::new();
        let turn_id = TurnId::new();
        msgs.into_iter()
            .enumerate()
            .map(|(i, msg)| Event {
                id,
                log_position: i as u64 + 1,
                thread_id,
                turn_id: Some(turn_id),
                timestamp_ms: 100 + i as u64,
                msg,
            })
            .collect()
    }

    fn streamed_turn() -> Vec<Event> {
        let message_id = MessageId::new();
        let delta = |text: &str| {
            EventMsg::AssistantMessageDelta(AssistantMessageDeltaEvent {
                message_id,
                delta: text.into(),
            })
        };
        events(vec![
            EventMsg::TurnStarted(TurnStartedEvent {}),
            EventMsg::AssistantMessageStart(AssistantMessageStartEvent { message_id }),
            delta("hello "),
            delta("world"),
            EventMsg::AssistantMessage(AssistantMessageEvent {
                message_id,
                content: Some("hello world".into()),
                tool_calls: vec![],
            }),
            EventMsg::TokenCount(TokenCountEvent {
                usage: Default::default(),
            }),
            EventMsg::TurnComplete(TurnCompleteEvent {
                last_agent_message: Some("hello world".into()),
                finish_reason: Default::default(),
            }),
        ])
    }

    fn render(adapter: &mut CliEventAdapter, input: &[Event]) -> Vec<String> {
        input
            .iter()
            .flat_map(|event| adapter.adapt(event))
            .map(|output| output.to_string())
            .collect()
    }

    fn every_event() -> Vec<Event> {
        events(vec![
            EventMsg::SessionConfigured(SessionConfiguredEvent {
                thread_id: ThreadId::new(),
                model: "m".into(),
            }),
            EventMsg::TurnStarted(TurnStartedEvent {}),
            EventMsg::UserMessage(UserMessageEvent {
                name: None,
                content: "hi".into(),
            }),
            EventMsg::EnvironmentContextUpdated(EnvironmentContextUpdatedEvent {
                context: EnvironmentContext::Unavailable,
            }),
            EventMsg::SystemReminder(SystemReminderEvent {
                iteration: 0,
                reminder: SystemReminder::ToolLimitReached,
            }),
            EventMsg::ModelRequestStarted(ModelRequestStartedEvent {
                iteration: 0,
                allow_tools: true,
            }),
            EventMsg::AssistantMessageStart(AssistantMessageStartEvent {
                message_id: MessageId::new(),
            }),
            EventMsg::AssistantMessageDelta(AssistantMessageDeltaEvent {
                message_id: MessageId::new(),
                delta: "x".into(),
            }),
            EventMsg::AssistantMessage(AssistantMessageEvent {
                message_id: MessageId::new(),
                content: None,
                tool_calls: vec![ToolCall::new("c".into(), "t", json!({}))],
            }),
            EventMsg::ModelRequestCompleted(ModelRequestCompletedEvent {
                iteration: 0,
                usage: Default::default(),
            }),
            EventMsg::ToolCallBegin(ToolCallBeginEvent {
                tool_call_id: "c".into(),
                tool_name: "t".into(),
                arguments: json!({}),
            }),
            EventMsg::ToolCallEnd(ToolCallEndEvent {
                result: ToolResult::completed(&ToolCall::new("c".into(), "t", json!({})), "ok"),
                duration_ms: Some(1),
            }),
            EventMsg::TokenCount(TokenCountEvent {
                usage: Default::default(),
            }),
            EventMsg::Warning(WarningEvent {
                message: "w".into(),
            }),
            EventMsg::Error(ErrorEvent {
                message: "e".into(),
            }),
            EventMsg::TurnAborted(TurnAbortedEvent {
                reason: TurnAbortReason::Interrupted,
                message: None,
            }),
            EventMsg::TurnComplete(TurnCompleteEvent {
                last_agent_message: None,
                finish_reason: Default::default(),
            }),
        ])
    }

    fn tool_end(status: ToolCallStatus, output: &str) -> Event {
        let call = ToolCall::new("c".into(), "lookup", json!({}));
        let result = match status {
            ToolCallStatus::Completed => ToolResult::completed(&call, output),
            _ => ToolResult {
                status,
                ..ToolResult::completed(&call, output)
            },
        };
        events(vec![EventMsg::ToolCallEnd(ToolCallEndEvent {
            result,
            duration_ms: None,
        })])
        .remove(0)
    }

    fn tool_begin(arguments: serde_json::Value) -> Event {
        events(vec![EventMsg::ToolCallBegin(ToolCallBeginEvent {
            tool_call_id: "c".into(),
            tool_name: "lookup".into(),
            arguments,
        })])
        .remove(0)
    }

    #[test]
    fn streamed_turn_renders_only_the_final_assistant_message() {
        assert_eq!(
            render(&mut CliEventAdapter::new(), &streamed_turn()),
            ["hello world"]
        );
    }

    #[test]
    fn default_adapter_renders_only_tool_calls_results_and_assistant_text() {
        let mut input = every_event();
        input.push(
            events(vec![EventMsg::AssistantMessage(AssistantMessageEvent {
                message_id: MessageId::new(),
                content: Some("answer".into()),
                tool_calls: vec![],
            })])
            .remove(0),
        );
        // `every_event` contains a tool-call-only assistant message, which is skipped.
        assert_eq!(
            render(&mut CliEventAdapter::new(), &input),
            ["tool call: t()", "tool result: t: ok", "answer"]
        );
    }

    #[test]
    fn tool_calls_render_name_and_arguments() {
        let mut adapter = CliEventAdapter::new();
        assert_eq!(
            render(
                &mut adapter,
                &[
                    tool_begin(json!({"q": "rust"})),
                    tool_begin(json!({})),
                    tool_begin(serde_json::Value::Null),
                    tool_begin(json!(["a"])),
                ]
            ),
            [
                r#"tool call: lookup({"q":"rust"})"#,
                "tool call: lookup()",
                "tool call: lookup()",
                r#"tool call: lookup(["a"])"#,
            ]
        );
    }

    #[test]
    fn tool_results_show_status_only_when_not_completed() {
        let mut adapter = CliEventAdapter::new();
        assert_eq!(
            render(
                &mut adapter,
                &[
                    tool_end(ToolCallStatus::Completed, "42"),
                    tool_end(ToolCallStatus::Failed, "boom"),
                    tool_end(ToolCallStatus::NotFound, "no such tool"),
                ]
            ),
            [
                "tool result: lookup: 42",
                "tool result: lookup (failed): boom",
                "tool result: lookup (not found): no such tool",
            ]
        );
    }

    #[test]
    fn blank_assistant_content_is_skipped() {
        let message = |content: Option<&str>| {
            events(vec![EventMsg::AssistantMessage(AssistantMessageEvent {
                message_id: MessageId::new(),
                content: content.map(Into::into),
                tool_calls: vec![],
            })])
            .remove(0)
        };
        assert!(render(
            &mut CliEventAdapter::new(),
            &[message(None), message(Some("")), message(Some("  \n"))]
        )
        .is_empty());
    }

    #[test]
    fn including_deltas_adds_raw_delta_text() {
        let mut adapter = CliEventAdapter::with_options(CliAdapterOptions {
            include_deltas: true,
        });
        assert!(adapter.options().include_deltas);
        assert_eq!(
            render(&mut adapter, &streamed_turn()),
            ["hello ", "world", "hello world"]
        );
    }

    #[test]
    fn finish_emits_nothing() {
        let mut adapter = CliEventAdapter::new();
        render(&mut adapter, &streamed_turn());
        assert!(adapter.finish().is_empty());
        assert!(!CliAdapterOptions::default().include_deltas);
    }
}
