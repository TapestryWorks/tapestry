use tapestry_protocol::{FinishReason, SystemReminder, ToolCall, ToolResult};

use crate::error::CoreError;

use super::{Agent, TurnStep};

#[derive(Debug)]
enum Phase {
    NeedModel,
    AwaitTools,
    Finished(FinishReason),
}

/// Default ReAct policy, allowing up to `max_tool_iterations` tool iterations
/// followed by one forced final model call.
pub struct ReactAgent {
    max_tool_iterations: u32,
    iteration: u64,
    phase: Phase,
    last_agent_message: Option<String>,
}

impl ReactAgent {
    pub fn new(max_tool_iterations: u32) -> Self {
        Self {
            max_tool_iterations,
            iteration: 0,
            phase: Phase::NeedModel,
            last_agent_message: None,
        }
    }
}

impl Agent for ReactAgent {
    fn next_step(&self) -> TurnStep {
        match self.phase {
            Phase::NeedModel => {
                let allow_tools = self.iteration < u64::from(self.max_tool_iterations);
                TurnStep::CallModel {
                    allow_tools,
                    reminder: (!allow_tools).then_some(SystemReminder::ToolLimitReached),
                }
            }
            Phase::AwaitTools => TurnStep::Done {
                finish: FinishReason::Completed,
                last_agent_message: self.last_agent_message.clone(),
            },
            Phase::Finished(finish) => TurnStep::Done {
                finish,
                last_agent_message: self.last_agent_message.clone(),
            },
        }
    }

    fn on_model_output(
        &mut self,
        content: Option<&str>,
        tool_calls: &[ToolCall],
    ) -> Result<Vec<ToolCall>, CoreError> {
        let Phase::NeedModel = self.phase else {
            return Err(CoreError::Internal(
                "model output without a model call".into(),
            ));
        };
        let allow_tools = self.iteration < u64::from(self.max_tool_iterations);
        self.iteration += 1;

        if let Some(text) = content {
            self.last_agent_message = Some(text.into());
        }
        let calls = if allow_tools {
            tool_calls.to_vec()
        } else {
            Vec::new()
        };
        self.phase = if !allow_tools {
            Phase::Finished(FinishReason::ExceedMaxIters)
        } else if calls.is_empty() {
            Phase::Finished(FinishReason::Completed)
        } else {
            Phase::AwaitTools
        };
        Ok(calls)
    }

    fn on_tool_results(&mut self, _results: &[ToolResult]) -> Result<(), CoreError> {
        let Phase::AwaitTools = self.phase else {
            return Err(CoreError::Internal(
                "tool results without tool calls".into(),
            ));
        };
        self.phase = Phase::NeedModel;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use tapestry_protocol::{ToolCallId, ToolCallStatus};

    use super::*;

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
        let mut agent = ReactAgent::new(3);
        assert_eq!(
            agent.next_step(),
            TurnStep::CallModel {
                allow_tools: true,
                reminder: None
            }
        );
        assert!(agent
            .on_model_output(Some("hello"), &[])
            .unwrap()
            .is_empty());
        assert_eq!(
            agent.next_step(),
            TurnStep::Done {
                finish: FinishReason::Completed,
                last_agent_message: Some("hello".into())
            }
        );
    }

    #[test]
    fn tool_loop_returns_entries_in_order() {
        let mut agent = ReactAgent::new(3);
        let a = call("echo");
        let b = call("echo");
        assert_eq!(
            agent
                .on_model_output(None, &[a.clone(), b.clone()])
                .unwrap(),
            [a.clone(), b.clone()]
        );
        agent
            .on_tool_results(&[
                outcome(&a, ToolCallStatus::Completed),
                outcome(&b, ToolCallStatus::Completed),
            ])
            .unwrap();
        assert_eq!(
            agent.next_step(),
            TurnStep::CallModel {
                allow_tools: true,
                reminder: None
            }
        );
        agent.on_model_output(Some("fin"), &[]).unwrap();
        assert_eq!(
            agent.next_step(),
            TurnStep::Done {
                finish: FinishReason::Completed,
                last_agent_message: Some("fin".into())
            }
        );
    }

    #[test]
    fn failed_and_not_found_results_feed_back_and_continue() {
        let mut agent = ReactAgent::new(3);
        let a = call("echo");
        let b = call("ghost");
        agent
            .on_model_output(None, &[a.clone(), b.clone()])
            .unwrap();
        agent
            .on_tool_results(&[
                outcome(&a, ToolCallStatus::Failed),
                outcome(&b, ToolCallStatus::NotFound),
            ])
            .unwrap();
        assert!(matches!(agent.next_step(), TurnStep::CallModel { .. }));
    }

    #[test]
    fn exceeding_limit_forces_finish_with_reminder() {
        let mut agent = ReactAgent::new(1);
        let a = call("echo");
        agent
            .on_model_output(None, std::slice::from_ref(&a))
            .unwrap();
        agent
            .on_tool_results(&[outcome(&a, ToolCallStatus::Completed)])
            .unwrap();
        assert_eq!(
            agent.next_step(),
            TurnStep::CallModel {
                allow_tools: false,
                reminder: Some(SystemReminder::ToolLimitReached)
            }
        );
        assert!(agent
            .on_model_output(Some("summary"), &[])
            .unwrap()
            .is_empty());
        assert_eq!(
            agent.next_step(),
            TurnStep::Done {
                finish: FinishReason::ExceedMaxIters,
                last_agent_message: Some("summary".into())
            }
        );
    }

    #[test]
    fn forced_finish_ignores_tool_calls() {
        let mut agent = ReactAgent::new(0);
        assert_eq!(
            agent.next_step(),
            TurnStep::CallModel {
                allow_tools: false,
                reminder: Some(SystemReminder::ToolLimitReached)
            }
        );
        assert!(agent
            .on_model_output(None, &[call("echo")])
            .unwrap()
            .is_empty());
        assert_eq!(
            agent.next_step(),
            TurnStep::Done {
                finish: FinishReason::ExceedMaxIters,
                last_agent_message: None
            }
        );

        let mut agent = ReactAgent::new(0);
        agent
            .on_model_output(Some("text"), &[call("echo")])
            .unwrap();
        assert_eq!(
            agent.next_step(),
            TurnStep::Done {
                finish: FinishReason::ExceedMaxIters,
                last_agent_message: Some("text".into())
            }
        );
    }

    #[test]
    fn rejects_outputs_and_results_in_the_wrong_phase() {
        let mut agent = ReactAgent::new(3);
        assert!(agent.on_tool_results(&[]).is_err());
        agent.on_model_output(Some("done"), &[]).unwrap();
        assert!(agent.on_model_output(None, &[]).is_err());

        let mut agent = ReactAgent::new(3);
        let call = call("echo");
        agent.on_model_output(None, &[call]).unwrap();
        assert!(agent.on_tool_results(&[]).is_ok());
        assert!(agent.on_tool_results(&[]).is_err());
    }
}
