//! Pure, synchronous decision logic for a single ReAct turn.
//!
//! [`TurnMachine`] performs no IO: the driver ([`crate::agent::AgentLoop`])
//! asks for the [`TurnStep`], performs it, and feeds the outcome back.

use tapestry_protocol::{FinishReason, ToolCallId, ToolCallStatus};

use crate::error::CoreError;
use crate::history::{fill_missing_tool_results, HistoryItem, ToolCall};

/// Temporary instruction for the forced final call; never stored in history.
pub const FORCED_FINISH_REMINDER: &str =
    "The tool call limit has been reached. Do not call any tools; summarize your progress and answer now.";

/// What the driver must do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnStep {
    CallModel {
        iteration: u32,
        allow_tools: bool,
        reminder: Option<String>,
    },
    RunTools {
        calls: Vec<ToolCall>,
    },
    Done {
        finish: FinishReason,
        last_agent_message: Option<String>,
    },
}

/// Result of one executed tool call (or a synthesized one, e.g. `not_found`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutcome {
    pub call_id: ToolCallId,
    pub tool_name: String,
    pub output: String,
    pub status: ToolCallStatus,
}

/// What `on_model_output` did with the model output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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

pub struct TurnMachine<'a> {
    history: &'a mut Vec<HistoryItem>,
    max_tool_iterations: u32,
    iteration: u32,
    phase: Phase,
    last_agent_message: Option<String>,
}

impl<'a> TurnMachine<'a> {
    /// `history` must already contain the user message that starts the turn.
    pub fn new(history: &'a mut Vec<HistoryItem>, max_tool_iterations: u32) -> Self {
        Self {
            history,
            max_tool_iterations,
            iteration: 0,
            phase: Phase::NeedModel,
            last_agent_message: None,
        }
    }

    pub fn history(&self) -> &[HistoryItem] {
        self.history
    }

    pub fn next_step(&self) -> TurnStep {
        match &self.phase {
            Phase::NeedModel => {
                let allow_tools = self.iteration < self.max_tool_iterations;
                TurnStep::CallModel {
                    iteration: self.iteration,
                    allow_tools,
                    reminder: (!allow_tools).then(|| FORCED_FINISH_REMINDER.to_string()),
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

    pub fn on_model_output(
        &mut self,
        content: Option<String>,
        tool_calls: Vec<ToolCall>,
    ) -> Result<ModelOutputEffect, CoreError> {
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
        if content.is_some() || !tool_calls.is_empty() {
            self.history
                .push(HistoryItem::assistant(content, tool_calls.clone()));
        }

        self.phase = if !allow_tools {
            Phase::Finished(FinishReason::ExceedMaxIters)
        } else if tool_calls.is_empty() {
            Phase::Finished(FinishReason::Completed)
        } else {
            Phase::NeedTools(tool_calls)
        };
        Ok(effect)
    }

    /// Record tool results, in call order, and schedule the next model call.
    pub fn on_tool_outcomes(&mut self, outcomes: Vec<ToolOutcome>) -> Result<(), CoreError> {
        let Phase::NeedTools(calls) = &self.phase else {
            return Err(CoreError::Internal(
                "tool outcomes without tool calls".into(),
            ));
        };
        let matches = calls.len() == outcomes.len()
            && calls.iter().zip(&outcomes).all(|(c, o)| c.id == o.call_id);
        if !matches {
            return Err(CoreError::Internal(
                "tool outcomes do not match requested calls".into(),
            ));
        }

        for outcome in outcomes {
            self.history.push(HistoryItem::tool_result(
                outcome.call_id,
                outcome.tool_name,
                outcome.output,
                outcome.status,
            ));
        }
        self.phase = Phase::NeedModel;
        Ok(())
    }

    /// Make the history valid after an aborted turn.
    pub fn repair_history(&mut self) {
        fill_missing_tool_results(self.history);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    fn outcome(call: &ToolCall, status: ToolCallStatus) -> ToolOutcome {
        ToolOutcome {
            call_id: call.id,
            tool_name: call.name.clone(),
            output: "out".into(),
            status,
        }
    }

    fn start(history: &mut Vec<HistoryItem>) {
        history.push(HistoryItem::user(None, "hi"));
    }

    #[test]
    fn completes_without_tool_calls() {
        let mut history = Vec::new();
        start(&mut history);
        let mut m = TurnMachine::new(&mut history, 3);
        assert_eq!(
            m.next_step(),
            TurnStep::CallModel {
                iteration: 0,
                allow_tools: true,
                reminder: None
            }
        );
        m.on_model_output(Some("hello".into()), vec![]).unwrap();
        assert_eq!(
            m.next_step(),
            TurnStep::Done {
                finish: FinishReason::Completed,
                last_agent_message: Some("hello".into())
            }
        );
    }

    #[test]
    fn tool_loop_writes_history_in_order() {
        let mut history = Vec::new();
        start(&mut history);
        let mut m = TurnMachine::new(&mut history, 3);
        let a = ToolCall::new("echo", Value::Null);
        let b = ToolCall::new("echo", Value::Null);
        m.on_model_output(None, vec![a.clone(), b.clone()]).unwrap();
        assert_eq!(
            m.next_step(),
            TurnStep::RunTools {
                calls: vec![a.clone(), b.clone()]
            }
        );
        m.on_tool_outcomes(vec![
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
        m.on_model_output(Some("fin".into()), vec![]).unwrap();

        let h = m.history();
        assert_eq!(h.len(), 5);
        assert!(matches!(h[0], HistoryItem::User { .. }));
        assert!(
            matches!(&h[1], HistoryItem::Assistant { tool_calls, .. } if tool_calls.len() == 2)
        );
        assert!(matches!(&h[2], HistoryItem::ToolResult { call_id, .. } if *call_id == a.id));
        assert!(matches!(&h[3], HistoryItem::ToolResult { call_id, .. } if *call_id == b.id));
        assert!(matches!(&h[4], HistoryItem::Assistant { content: Some(c), .. } if c == "fin"));
    }

    #[test]
    fn failed_and_not_found_results_feed_back_and_continue() {
        let mut history = Vec::new();
        start(&mut history);
        let mut m = TurnMachine::new(&mut history, 3);
        let a = ToolCall::new("echo", Value::Null);
        let b = ToolCall::new("ghost", Value::Null);
        m.on_model_output(None, vec![a.clone(), b.clone()]).unwrap();
        m.on_tool_outcomes(vec![
            outcome(&a, ToolCallStatus::Failed),
            outcome(&b, ToolCallStatus::NotFound),
        ])
        .unwrap();
        assert!(matches!(m.next_step(), TurnStep::CallModel { .. }));
        assert!(matches!(
            &m.history()[2],
            HistoryItem::ToolResult {
                status: ToolCallStatus::Failed,
                ..
            }
        ));
        assert!(matches!(
            &m.history()[3],
            HistoryItem::ToolResult {
                status: ToolCallStatus::NotFound,
                ..
            }
        ));
    }

    #[test]
    fn exceeding_limit_forces_finish_without_storing_reminder() {
        let mut history = Vec::new();
        start(&mut history);
        let mut m = TurnMachine::new(&mut history, 1);
        let a = ToolCall::new("echo", Value::Null);
        m.on_model_output(None, vec![a.clone()]).unwrap();
        m.on_tool_outcomes(vec![outcome(&a, ToolCallStatus::Completed)])
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
        assert!(reminder.is_some());

        let effect = m.on_model_output(Some("summary".into()), vec![]).unwrap();
        assert_eq!(effect.ignored_tool_calls, 0);
        assert_eq!(
            m.next_step(),
            TurnStep::Done {
                finish: FinishReason::ExceedMaxIters,
                last_agent_message: Some("summary".into())
            }
        );
        assert!(!m.history().iter().any(
            |i| matches!(i, HistoryItem::User { content, .. } if content == FORCED_FINISH_REMINDER)
        ));
        assert_eq!(m.history().len(), 4);
    }

    #[test]
    fn forced_finish_ignores_tool_calls() {
        let mut history = Vec::new();
        start(&mut history);
        let mut m = TurnMachine::new(&mut history, 0);
        assert!(matches!(
            m.next_step(),
            TurnStep::CallModel {
                allow_tools: false,
                ..
            }
        ));
        let effect = m
            .on_model_output(None, vec![ToolCall::new("echo", Value::Null)])
            .unwrap();
        assert_eq!(effect.ignored_tool_calls, 1);
        assert!(matches!(
            m.next_step(),
            TurnStep::Done {
                finish: FinishReason::ExceedMaxIters,
                ..
            }
        ));
        assert!(!m.history().iter().any(
            |i| matches!(i, HistoryItem::Assistant { tool_calls, .. } if !tool_calls.is_empty())
        ));
    }

    #[test]
    fn mismatched_outcomes_are_rejected() {
        let mut history = Vec::new();
        start(&mut history);
        let mut m = TurnMachine::new(&mut history, 3);
        let a = ToolCall::new("echo", Value::Null);
        let other = ToolCall::new("echo", Value::Null);
        m.on_model_output(None, vec![a]).unwrap();
        assert!(m
            .on_tool_outcomes(vec![outcome(&other, ToolCallStatus::Completed)])
            .is_err());
        assert!(m.on_tool_outcomes(vec![]).is_err());
    }

    #[test]
    fn out_of_order_calls_are_rejected() {
        let mut history = Vec::new();
        start(&mut history);
        let mut m = TurnMachine::new(&mut history, 3);
        assert!(m.on_tool_outcomes(vec![]).is_err());
        m.on_model_output(Some("x".into()), vec![]).unwrap();
        assert!(m.on_model_output(None, vec![]).is_err());
    }

    #[test]
    fn repair_fills_unanswered_tool_calls() {
        let mut history = Vec::new();
        start(&mut history);
        let mut m = TurnMachine::new(&mut history, 3);
        let a = ToolCall::new("echo", Value::Null);
        let b = ToolCall::new("echo", Value::Null);
        m.on_model_output(None, vec![a.clone(), b.clone()]).unwrap();
        m.repair_history();

        for call in [&a, &b] {
            assert!(m.history().iter().any(|i| matches!(
                i,
                HistoryItem::ToolResult { call_id, status: ToolCallStatus::Failed, .. } if *call_id == call.id
            )));
        }
    }
}
