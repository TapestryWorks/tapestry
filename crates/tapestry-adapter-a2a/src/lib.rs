//! A2A 1.0 streaming-response adapter for Tapestry event streams.
//!
//! A2A 1.0 event mapping:
//!
//! | Tapestry event | A2A `StreamResponse` |
//! |---|---|
//! | `TurnStarted` | working `task` snapshot |
//! | assistant text delta | `artifactUpdate` (first chunk replaces, subsequent chunks append) |
//! | final assistant text | replacing `artifactUpdate` with `lastChunk: true` |
//! | `TurnComplete` | completed `statusUpdate` |
//! | `TurnAborted` | canceled `statusUpdate` |
//! | in-turn `Error` | failed `statusUpdate` with an agent message |
//! | stream end with an active task | failed `statusUpdate` from `finish` |
//! | all other events | ignored |
//!
//! This crate implements the outbound subset of A2A 1.0 JSON mapping only. It
//! does not implement transport, inbound methods, task storage, or agent cards.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use tapestry_protocol::{Event, EventAdapter, EventMsg, MessageId, ThreadId, TurnId};

/// A2A `StreamResponse` oneof. Serde's externally tagged representation
/// produces exactly one top-level field for each response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StreamResponse {
    Task(Task),
    StatusUpdate(TaskStatusUpdateEvent),
    ArtifactUpdate(TaskArtifactUpdateEvent),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub context_id: String,
    pub status: TaskStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatus {
    pub state: TaskState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskState {
    #[serde(rename = "TASK_STATE_WORKING")]
    Working,
    #[serde(rename = "TASK_STATE_COMPLETED")]
    Completed,
    #[serde(rename = "TASK_STATE_CANCELED")]
    Canceled,
    #[serde(rename = "TASK_STATE_FAILED")]
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub message_id: String,
    pub task_id: String,
    pub context_id: String,
    pub role: Role,
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    #[serde(rename = "ROLE_AGENT")]
    Agent,
}

/// The A2A text member of the `Part` oneof.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Part {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub artifact_id: String,
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatusUpdateEvent {
    pub task_id: String,
    pub context_id: String,
    pub status: TaskStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskArtifactUpdateEvent {
    pub task_id: String,
    pub context_id: String,
    pub artifact: Artifact,
    pub append: bool,
    pub last_chunk: bool,
}

/// Stateful adapter for one thread's event stream.
#[derive(Default)]
pub struct A2aAdapter {
    active: Vec<(TurnId, ThreadId)>,
    terminal: HashSet<TurnId>,
    artifact_started: HashSet<MessageId>,
}

impl A2aAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    fn is_active(&self, turn_id: TurnId) -> bool {
        self.active.iter().any(|(id, _)| *id == turn_id)
    }

    fn context_for(&self, turn_id: TurnId) -> Option<ThreadId> {
        self.active
            .iter()
            .find_map(|(id, thread_id)| (*id == turn_id).then_some(*thread_id))
    }

    fn status_update(
        &mut self,
        turn_id: TurnId,
        state: TaskState,
        message: Option<String>,
    ) -> Option<StreamResponse> {
        let context_id = self.context_for(turn_id)?;
        self.active.retain(|(id, _)| *id != turn_id);
        self.terminal.insert(turn_id);
        Some(StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
            task_id: turn_id.to_string(),
            context_id: context_id.to_string(),
            status: TaskStatus {
                state,
                message: message.map(|text| Message {
                    message_id: format!("{turn_id}:status"),
                    task_id: turn_id.to_string(),
                    context_id: context_id.to_string(),
                    role: Role::Agent,
                    parts: vec![Part { text }],
                }),
                timestamp: None,
            },
        }))
    }

    fn artifact(
        &self,
        event: &Event,
        message_id: MessageId,
        text: String,
        append: bool,
        last_chunk: bool,
    ) -> Option<StreamResponse> {
        let turn_id = event.turn_id?;
        if self.terminal.contains(&turn_id) || !self.is_active(turn_id) {
            return None;
        }
        Some(StreamResponse::ArtifactUpdate(TaskArtifactUpdateEvent {
            task_id: turn_id.to_string(),
            context_id: event.thread_id.to_string(),
            artifact: Artifact {
                artifact_id: message_id.to_string(),
                parts: vec![Part { text }],
            },
            append,
            last_chunk,
        }))
    }
}

impl EventAdapter for A2aAdapter {
    type Output = StreamResponse;

    fn adapt(&mut self, event: &Event) -> Vec<Self::Output> {
        if !matches!(event.msg, EventMsg::TurnStarted(_))
            && event
                .turn_id
                .is_some_and(|turn_id| self.terminal.contains(&turn_id))
        {
            return Vec::new();
        }

        match &event.msg {
            EventMsg::TurnStarted(_) => {
                let Some(turn_id) = event.turn_id else {
                    return Vec::new();
                };
                if self.terminal.contains(&turn_id) {
                    return Vec::new();
                }
                if !self.is_active(turn_id) {
                    self.active.push((turn_id, event.thread_id));
                }
                vec![StreamResponse::Task(Task {
                    id: turn_id.to_string(),
                    context_id: event.thread_id.to_string(),
                    status: TaskStatus {
                        state: TaskState::Working,
                        message: None,
                        timestamp: None,
                    },
                })]
            }
            EventMsg::AssistantMessageDelta(delta) => {
                let Some(turn_id) = event.turn_id else {
                    return Vec::new();
                };
                if self.terminal.contains(&turn_id) || !self.is_active(turn_id) {
                    return Vec::new();
                }
                let append = self.artifact_started.contains(&delta.message_id);
                self.artifact_started.insert(delta.message_id);
                self.artifact(event, delta.message_id, delta.delta.clone(), append, false)
                    .into_iter()
                    .collect()
            }
            EventMsg::AssistantMessage(message) => {
                let Some(text) = message.content.clone().filter(|text| !text.is_empty()) else {
                    return Vec::new();
                };
                self.artifact_started.insert(message.message_id);
                self.artifact(event, message.message_id, text, false, true)
                    .into_iter()
                    .collect()
            }
            EventMsg::TurnComplete(_) => event
                .turn_id
                .and_then(|turn_id| self.status_update(turn_id, TaskState::Completed, None))
                .into_iter()
                .collect(),
            EventMsg::TurnAborted(aborted) => event
                .turn_id
                .and_then(|turn_id| {
                    self.status_update(
                        turn_id,
                        TaskState::Canceled,
                        Some(
                            aborted
                                .message
                                .clone()
                                .unwrap_or_else(|| "turn aborted".into()),
                        ),
                    )
                })
                .into_iter()
                .collect(),
            EventMsg::Error(error) => event
                .turn_id
                .and_then(|turn_id| {
                    self.status_update(turn_id, TaskState::Failed, Some(error.message.clone()))
                })
                .into_iter()
                .collect(),
            _ => Vec::new(),
        }
    }

    fn finish(&mut self) -> Vec<Self::Output> {
        let active = std::mem::take(&mut self.active);
        active
            .into_iter()
            .map(|(turn_id, thread_id)| {
                self.terminal.insert(turn_id);
                StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
                    task_id: turn_id.to_string(),
                    context_id: thread_id.to_string(),
                    status: TaskStatus {
                        state: TaskState::Failed,
                        message: Some(Message {
                            message_id: format!("{turn_id}:status"),
                            task_id: turn_id.to_string(),
                            context_id: thread_id.to_string(),
                            role: Role::Agent,
                            parts: vec![Part {
                                text: "event stream ended before task completed".into(),
                            }],
                        }),
                        timestamp: None,
                    },
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use tapestry_protocol::{
        AssistantMessageDeltaEvent, AssistantMessageEvent, AssistantMessageStartEvent, ErrorEvent,
        Event, FinishReason, SubmissionId, ThreadId, ToolCall, ToolCallId, TurnAbortReason,
        TurnAbortedEvent, TurnCompleteEvent, TurnStartedEvent,
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

    fn turn() -> TurnId {
        TurnId::new()
    }

    fn response_value(response: &StreamResponse) -> Value {
        serde_json::to_value(response).unwrap()
    }

    fn collect(adapter: &mut A2aAdapter, events: &[Event]) -> Vec<StreamResponse> {
        events
            .iter()
            .flat_map(|event| adapter.adapt(event))
            .collect()
    }

    #[test]
    fn line_format_is_oneof_with_camel_case_and_a2a_enum_names() {
        let values = [
            response_value(&StreamResponse::Task(Task {
                id: "turn".into(),
                context_id: "thread".into(),
                status: TaskStatus {
                    state: TaskState::Working,
                    message: None,
                    timestamp: None,
                },
            })),
            response_value(&StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
                task_id: "turn".into(),
                context_id: "thread".into(),
                status: TaskStatus {
                    state: TaskState::Completed,
                    message: None,
                    timestamp: None,
                },
            })),
            response_value(&StreamResponse::ArtifactUpdate(TaskArtifactUpdateEvent {
                task_id: "turn".into(),
                context_id: "thread".into(),
                artifact: Artifact {
                    artifact_id: "message".into(),
                    parts: vec![Part { text: "hi".into() }],
                },
                append: false,
                last_chunk: true,
            })),
        ];
        for value in values {
            assert_eq!(value.as_object().unwrap().len(), 1);
            assert_eq!(
                serde_json::from_value::<StreamResponse>(value.clone()).unwrap(),
                match value.as_object().unwrap().keys().next().unwrap().as_str() {
                    "task" => StreamResponse::Task(Task {
                        id: "turn".into(),
                        context_id: "thread".into(),
                        status: TaskStatus {
                            state: TaskState::Working,
                            message: None,
                            timestamp: None,
                        },
                    }),
                    "statusUpdate" => StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
                        task_id: "turn".into(),
                        context_id: "thread".into(),
                        status: TaskStatus {
                            state: TaskState::Completed,
                            message: None,
                            timestamp: None,
                        },
                    }),
                    _ => StreamResponse::ArtifactUpdate(TaskArtifactUpdateEvent {
                        task_id: "turn".into(),
                        context_id: "thread".into(),
                        artifact: Artifact {
                            artifact_id: "message".into(),
                            parts: vec![Part { text: "hi".into() }],
                        },
                        append: false,
                        last_chunk: true,
                    }),
                }
            );
        }
        assert_eq!(json!(TaskState::Completed), "TASK_STATE_COMPLETED");
        assert_eq!(json!(Role::Agent), "ROLE_AGENT");
        assert_eq!(
            response_value(&StreamResponse::ArtifactUpdate(TaskArtifactUpdateEvent {
                task_id: "t".into(),
                context_id: "c".into(),
                artifact: Artifact {
                    artifact_id: "a".into(),
                    parts: vec![Part { text: "x".into() }],
                },
                append: true,
                last_chunk: false,
            }))["artifactUpdate"]["lastChunk"],
            false
        );
    }

    #[test]
    fn turn_start_and_streaming_artifact_chunks_map_correctly() {
        let turn_id = turn();
        let message_id = MessageId::new();
        let mut adapter = A2aAdapter::new();
        let output = collect(
            &mut adapter,
            &[
                event(1, Some(turn_id), EventMsg::TurnStarted(TurnStartedEvent {})),
                event(
                    2,
                    Some(turn_id),
                    EventMsg::AssistantMessageStart(AssistantMessageStartEvent { message_id }),
                ),
                event(
                    3,
                    Some(turn_id),
                    EventMsg::AssistantMessageDelta(AssistantMessageDeltaEvent {
                        message_id,
                        delta: "Hel".into(),
                    }),
                ),
                event(
                    4,
                    Some(turn_id),
                    EventMsg::AssistantMessageDelta(AssistantMessageDeltaEvent {
                        message_id,
                        delta: "lo".into(),
                    }),
                ),
                event(
                    5,
                    Some(turn_id),
                    EventMsg::AssistantMessage(AssistantMessageEvent {
                        message_id,
                        content: Some("Hello".into()),
                        tool_calls: vec![],
                    }),
                ),
            ],
        );
        assert!(
            matches!(&output[0], StreamResponse::Task(Task { id, status: TaskStatus { state: TaskState::Working, .. }, .. }) if id == &turn_id.to_string())
        );
        let updates: Vec<_> = output[1..]
            .iter()
            .map(|item| match item {
                StreamResponse::ArtifactUpdate(update) => update,
                _ => panic!("expected artifact update"),
            })
            .collect();
        assert_eq!(
            updates.iter().map(|u| u.append).collect::<Vec<_>>(),
            [false, true, false]
        );
        assert_eq!(
            updates.iter().map(|u| u.last_chunk).collect::<Vec<_>>(),
            [false, false, true]
        );
        assert_eq!(updates[0].artifact.parts[0].text, "Hel");
        assert_eq!(updates[1].artifact.parts[0].text, "lo");
        assert_eq!(updates[2].artifact.parts[0].text, "Hello");
        assert!(updates
            .iter()
            .all(|u| u.artifact.artifact_id == message_id.to_string()));
    }

    #[test]
    fn non_streamed_text_is_one_final_artifact_and_empty_text_is_ignored() {
        let turn_id = turn();
        let mut adapter = A2aAdapter::new();
        let output = collect(
            &mut adapter,
            &[
                event(1, Some(turn_id), EventMsg::TurnStarted(TurnStartedEvent {})),
                event(
                    2,
                    Some(turn_id),
                    EventMsg::AssistantMessage(AssistantMessageEvent {
                        message_id: MessageId::new(),
                        content: Some("hi".into()),
                        tool_calls: vec![],
                    }),
                ),
            ],
        );
        assert!(
            matches!(&output[1], StreamResponse::ArtifactUpdate(u) if !u.append && u.last_chunk && u.artifact.parts[0].text == "hi")
        );
        assert!(adapter
            .adapt(&event(
                3,
                Some(turn_id),
                EventMsg::AssistantMessage(AssistantMessageEvent {
                    message_id: MessageId::new(),
                    content: None,
                    tool_calls: vec![],
                })
            ))
            .is_empty());
    }

    #[test]
    fn terminal_statuses_and_post_terminal_events() {
        let turn_id = turn();
        let mut adapter = A2aAdapter::new();
        let output = collect(
            &mut adapter,
            &[
                event(1, Some(turn_id), EventMsg::TurnStarted(TurnStartedEvent {})),
                event(
                    2,
                    Some(turn_id),
                    EventMsg::TurnComplete(TurnCompleteEvent {
                        last_agent_message: None,
                        finish_reason: FinishReason::Completed,
                    }),
                ),
                event(
                    3,
                    Some(turn_id),
                    EventMsg::Error(ErrorEvent {
                        message: "late".into(),
                    }),
                ),
            ],
        );
        assert_eq!(output.len(), 2);
        assert!(
            matches!(&output[1], StreamResponse::StatusUpdate(s) if s.status.state == TaskState::Completed)
        );

        let turn_id = turn();
        let mut adapter = A2aAdapter::new();
        let output = collect(
            &mut adapter,
            &[
                event(1, Some(turn_id), EventMsg::TurnStarted(TurnStartedEvent {})),
                event(
                    2,
                    Some(turn_id),
                    EventMsg::TurnAborted(TurnAbortedEvent {
                        reason: TurnAbortReason::Interrupted,
                        message: Some("stop".into()),
                    }),
                ),
            ],
        );
        assert!(
            matches!(&output[1], StreamResponse::StatusUpdate(s) if s.status.state == TaskState::Canceled)
        );

        let turn_id = turn();
        let mut adapter = A2aAdapter::new();
        let output = collect(
            &mut adapter,
            &[
                event(1, Some(turn_id), EventMsg::TurnStarted(TurnStartedEvent {})),
                event(
                    2,
                    Some(turn_id),
                    EventMsg::Error(ErrorEvent {
                        message: "boom".into(),
                    }),
                ),
            ],
        );
        assert!(
            matches!(&output[1], StreamResponse::StatusUpdate(s) if s.status.state == TaskState::Failed && s.status.message.as_ref().unwrap().parts[0].text == "boom")
        );
    }

    #[test]
    fn irrelevant_events_and_errors_without_turn_id_are_ignored() {
        let mut adapter = A2aAdapter::new();
        assert!(adapter
            .adapt(&event(
                1,
                None,
                EventMsg::Error(ErrorEvent {
                    message: "x".into()
                })
            ))
            .is_empty());
        assert!(adapter
            .adapt(&event(
                2,
                Some(turn()),
                EventMsg::ToolCallBegin(tapestry_protocol::ToolCallBeginEvent {
                    tool_call_id: ToolCallId::from("c"),
                    tool_name: "echo".into(),
                    arguments: json!({}),
                },)
            ))
            .is_empty());
    }

    #[test]
    fn finish_fails_active_tasks_once() {
        let turn_id = turn();
        let mut adapter = A2aAdapter::new();
        adapter.adapt(&event(
            1,
            Some(turn_id),
            EventMsg::TurnStarted(TurnStartedEvent {}),
        ));
        assert!(
            matches!(adapter.finish().as_slice(), [StreamResponse::StatusUpdate(s)] if s.status.state == TaskState::Failed)
        );
        assert!(adapter.finish().is_empty());
        assert!(adapter
            .adapt(&event(
                2,
                Some(turn_id),
                EventMsg::TurnComplete(TurnCompleteEvent {
                    last_agent_message: None,
                    finish_reason: FinishReason::Completed,
                })
            ))
            .is_empty());
    }

    #[test]
    fn tool_only_message_has_no_artifact_and_wire_has_single_key() {
        let turn_id = turn();
        let call = ToolCall::new(ToolCallId::from("c"), "echo", json!({}));
        let mut adapter = A2aAdapter::new();
        let output = collect(
            &mut adapter,
            &[
                event(1, Some(turn_id), EventMsg::TurnStarted(TurnStartedEvent {})),
                event(
                    2,
                    Some(turn_id),
                    EventMsg::AssistantMessage(AssistantMessageEvent {
                        message_id: MessageId::new(),
                        content: None,
                        tool_calls: vec![call],
                    }),
                ),
            ],
        );
        assert_eq!(output.len(), 1);
        assert_eq!(response_value(&output[0]).as_object().unwrap().len(), 1);
        assert!(matches!(output[0], StreamResponse::Task(_)));
    }
}
