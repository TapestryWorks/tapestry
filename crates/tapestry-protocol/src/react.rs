//! Internal messages between the ReAct driver and its state machine.

use serde::{Deserialize, Serialize};

use crate::common::FinishReason;
use crate::history::SystemReminder;
use crate::tool::ToolCall;

/// What the driver must do next.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TurnStep {
    CallModel {
        iteration: u32,
        allow_tools: bool,
        /// Appended to this request's history copy only.
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

#[cfg(test)]
mod tests {
    use serde_json::json;

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
}
