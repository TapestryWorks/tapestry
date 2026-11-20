//! Pure, synchronous decision logic for a single ReAct turn.
//!
//! [`TurnMachine`] performs no IO and owns no history: the driver
//! ([`crate::agent::AgentLoop`]) asks for the [`TurnStep`], performs it, feeds
//! the outcome back, and records whatever the machine returns as events.

use serde::{Deserialize, Serialize};
use tapestry_protocol::{
    AssistantMessageEvent, FinishReason, MessageId, SystemReminder, ToolCall, ToolResult,
};

use crate::error::CoreError;

/// What the driver must do next.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TurnStep {
    CallModel {
        iteration: u32,
        allow_tools: bool,
        /// Recorded as an event before the request snapshot is read.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reminder: Option<SystemReminder>,
    },
    RunTools {
        calls: Vec<ToolCall>,
    },
    Done {
        finish: FinishReason,
        last_agent_message: Option<String>,
    },
}

/// What the state machine did with a model output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ModelOutputEffect {
    /// Tool calls dropped because the forced final call must not use tools.
    pub ignored_tool_calls: usize,
}

#[derive(Debug)]
enum Phase {
    NeedModel,
    NeedTools(Vec<ToolCall>),
    Finished(FinishReason),
}

pub struct TurnMachine {
    max_tool_iterations: u32,
    iteration: u32,
    phase: Phase,
    last_agent_message: Option<String>,
}

impl TurnMachine {
    pub fn new(max_tool_iterations: u32) -> Self {
        Self {
            max_tool_iterations,
            iteration: 0,
            phase: Phase::NeedModel,
            last_agent_message: None,
        }
    }

    pub fn next_step(&self) -> TurnStep {
        match &self.phase {
            Phase::NeedModel => {
                let allow_tools = self.iteration < self.max_tool_iterations;
                TurnStep::CallModel {
                    iteration: self.iteration,
                    allow_tools,
                    reminder: (!allow_tools).then_some(SystemReminder::ToolLimitReached),
                }
            }
            Phase::NeedTools(calls) => TurnStep::RunTools {
                calls: calls.clone(),
            },
            Phase::Finished(finish) => TurnStep::Done {
                finish: *finish,
                last_agent_message: self.last_agent_message.clone(),
            },
        }
    }

    /// Returns the assistant entry to record (none when the output is empty
    /// after dropping tool calls the forced final call must not make).
    ///
    /// `message_id` identifies the resulting assistant message; the caller has
    /// already resolved `content` (final text, or concatenated deltas).
    pub fn on_model_output(
        &mut self,
        message_id: MessageId,
        content: Option<String>,
        tool_calls: Vec<ToolCall>,
    ) -> Result<(Option<AssistantMessageEvent>, ModelOutputEffect), CoreError> {
        let TurnStep::CallModel { allow_tools, .. } = self.next_step() else {
            return Err(CoreError::Internal(
                "model output without a model call".into(),
            ));
        };
        self.iteration += 1;

        let mut effect = ModelOutputEffect::default();
        let tool_calls = if allow_tools {
            tool_calls
        } else {
            effect.ignored_tool_calls = tool_calls.len();
            Vec::new()
        };

        if let Some(text) = &content {
            self.last_agent_message = Some(text.clone());
        }
        self.phase = if !allow_tools {
            Phase::Finished(FinishReason::ExceedMaxIters)
        } else if tool_calls.is_empty() {
            Phase::Finished(FinishReason::Completed)
        } else {
            Phase::NeedTools(tool_calls.clone())
        };
        let message =
            (content.is_some() || !tool_calls.is_empty()).then_some(AssistantMessageEvent {
                message_id,
                content,
                tool_calls,
            });
        Ok((message, effect))
    }

    /// Accept tool results, in call order, and schedule the next model call.
    pub fn on_tool_results(&mut self, results: &[ToolResult]) -> Result<(), CoreError> {
        let Phase::NeedTools(calls) = &self.phase else {
            return Err(CoreError::Internal(
                "tool results without tool calls".into(),
            ));
        };
        let matches = calls.len() == results.len()
            && calls.iter().zip(results).all(|(c, r)| c.id == r.call_id);
        if !matches {
            return Err(CoreError::Internal(
                "tool results do not match requested calls".into(),
            ));
        }

        self.phase = Phase::NeedModel;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use serde_json::Value;
    use tapestry_protocol::{ToolCallId, ToolCallStatus};

    use super::*;

    #[test]
    fn steps_round_trip_with_snake_case_tags() {
        let steps = [
            TurnStep::CallModel {
                iteration: 1,
                allow_tools: false,
                reminder: Some(SystemReminder::ToolLimitReached),
            },
            TurnStep::RunTools {
                calls: vec![ToolCall::new("c".into(), "echo", json!(null))],
            },
            TurnStep::Done {
                finish: FinishReason::Completed,
                last_agent_message: None,
            },
        ];
        for step in steps {
            let value = serde_json::to_value(&step).unwrap();
            assert_eq!(serde_json::from_value::<TurnStep>(value).unwrap(), step);
        }
        let value = serde_json::to_value(TurnStep::CallModel {
            iteration: 0,
            allow_tools: true,
            reminder: None,
        })
        .unwrap();
        assert_eq!(value["type"], "call_model");
        assert!(value.get("reminder").is_none());
    }

    #[test]
    fn effect_round_trip() {
        let effect = ModelOutputEffect {
            ignored_tool_calls: 2,
        };
        let json = serde_json::to_string(&effect).unwrap();
        assert_eq!(
            serde_json::from_str::<ModelOutputEffect>(&json).unwrap(),
            effect
        );
    }

    fn call(name: &str) -> ToolCall {
        ToolCall::new(ToolCallId::generate(), name, Value::Null)
    }

    fn outcome(call: &ToolCall, status: ToolCallStatus) -> ToolResult {
        ToolResult {
            call_id: call.id.clone(),
            tool_name: call.name.clone(),
            output: "out".into(),
            status,
        }
    }

    #[test]
    fn completes_without_tool_calls() {
        let mut m = TurnMachine::new(3);
        assert_eq!(
            m.next_step(),
            TurnStep::CallModel {
                iteration: 0,
                allow_tools: true,
                reminder: None
            }
        );
        let id = MessageId::new();
        let (message, _) = m.on_model_output(id, Some("hello".into()), vec![]).unwrap();
        assert_eq!(
            message,
            Some(AssistantMessageEvent {
                message_id: id,
                content: Some("hello".into()),
                tool_calls: vec![]
            })
        );
        assert_eq!(
            m.next_step(),
            TurnStep::Done {
                finish: FinishReason::Completed,
                last_agent_message: Some("hello".into())
            }
        );
    }

    #[test]
    fn tool_loop_returns_entries_in_order() {
        let mut m = TurnMachine::new(3);
        let a = call("echo");
        let b = call("echo");
        let (message, _) = m
            .on_model_output(MessageId::new(), None, vec![a.clone(), b.clone()])
            .unwrap();
        assert_eq!(message.unwrap().tool_calls, vec![a.clone(), b.clone()]);
        assert_eq!(
            m.next_step(),
            TurnStep::RunTools {
                calls: vec![a.clone(), b.clone()]
            }
        );
        m.on_tool_results(&[
            outcome(&a, ToolCallStatus::Completed),
            outcome(&b, ToolCallStatus::Completed),
        ])
        .unwrap();
        assert!(matches!(
            m.next_step(),
            TurnStep::CallModel {
                iteration: 1,
                allow_tools: true,
                ..
            }
        ));
        let (message, _) = m
            .on_model_output(MessageId::new(), Some("fin".into()), vec![])
            .unwrap();
        assert_eq!(message.unwrap().content.as_deref(), Some("fin"));
    }

    #[test]
    fn failed_and_not_found_results_feed_back_and_continue() {
        let mut m = TurnMachine::new(3);
        let a = call("echo");
        let b = call("ghost");
        m.on_model_output(MessageId::new(), None, vec![a.clone(), b.clone()])
            .unwrap();
        m.on_tool_results(&[
            outcome(&a, ToolCallStatus::Failed),
            outcome(&b, ToolCallStatus::NotFound),
        ])
        .unwrap();
        assert!(matches!(m.next_step(), TurnStep::CallModel { .. }));
    }

    #[test]
    fn exceeding_limit_forces_finish_with_reminder() {
        let mut m = TurnMachine::new(1);
        let a = call("echo");
        m.on_model_output(MessageId::new(), None, vec![a.clone()])
            .unwrap();
        m.on_tool_results(&[outcome(&a, ToolCallStatus::Completed)])
            .unwrap();

        let TurnStep::CallModel {
            iteration,
            allow_tools,
            reminder,
        } = m.next_step()
        else {
            panic!("expected model call");
        };
        assert_eq!(iteration, 1);
        assert!(!allow_tools);
        assert_eq!(reminder, Some(SystemReminder::ToolLimitReached));

        let (_, effect) = m
            .on_model_output(MessageId::new(), Some("summary".into()), vec![])
            .unwrap();
        assert_eq!(effect.ignored_tool_calls, 0);
        assert_eq!(
            m.next_step(),
            TurnStep::Done {
                finish: FinishReason::ExceedMaxIters,
                last_agent_message: Some("summary".into())
            }
        );
    }

    #[test]
    fn forced_finish_ignores_tool_calls() {
        let mut m = TurnMachine::new(0);
        assert!(matches!(
            m.next_step(),
            TurnStep::CallModel {
                allow_tools: false,
                ..
            }
        ));
        let (message, effect) = m
            .on_model_output(MessageId::new(), None, vec![call("echo")])
            .unwrap();
        assert_eq!(effect.ignored_tool_calls, 1);
        assert_eq!(message, None);
        assert!(matches!(
            m.next_step(),
            TurnStep::Done {
                finish: FinishReason::ExceedMaxIters,
                ..
            }
        ));

        let mut m = TurnMachine::new(0);
        let (message, _) = m
            .on_model_output(MessageId::new(), Some("text".into()), vec![call("echo")])
            .unwrap();
        let message = message.unwrap();
        assert_eq!(message.content.as_deref(), Some("text"));
        assert!(message.tool_calls.is_empty());
    }

    #[test]
    fn mismatched_results_are_rejected() {
        let mut m = TurnMachine::new(3);
        let a = call("echo");
        let other = call("echo");
        m.on_model_output(MessageId::new(), None, vec![a]).unwrap();
        assert!(m
            .on_tool_results(&[outcome(&other, ToolCallStatus::Completed)])
            .is_err());
        assert!(m.on_tool_results(&[]).is_err());
    }

    #[test]
    fn out_of_order_calls_are_rejected() {
        let mut m = TurnMachine::new(3);
        assert!(m.on_tool_results(&[]).is_err());
        m.on_model_output(MessageId::new(), Some("x".into()), vec![])
            .unwrap();
        assert!(m.on_model_output(MessageId::new(), None, vec![]).is_err());
    }
}
