use serde::{Deserialize, Serialize};

use crate::event::ThreadSettings;
use crate::ids::{SubmissionId, TurnId};

/// Submission Queue entry — a request from the client to the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Submission {
    /// Correlates with [`crate::event::Event`] responses.
    pub id: SubmissionId,
    pub op: Op,
}

impl Submission {
    pub fn new(op: Op) -> Self {
        Self {
            id: SubmissionId::new(),
            op,
        }
    }

    pub fn with_id(id: SubmissionId, op: Op) -> Self {
        Self { id, op }
    }
}

/// Client-to-agent operations.
///
/// Subset of Codex `Op` focused on turn lifecycle and thread configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Op {
    /// Configure a new or resumed thread session.
    Configure {
        model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        system_prompt: Option<String>,
    },
    /// Start processing user input for a turn.
    StartTurn {
        user_message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<TurnId>,
    },
    /// Abort the in-flight turn without shutting down the thread.
    Interrupt,
    /// Apply thread settings without starting a turn.
    ThreadSettings { settings: ThreadSettings },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::SubmissionId;

    #[test]
    fn start_turn_serialization() {
        let submission = Submission::with_id(
            SubmissionId::new(),
            Op::StartTurn {
                user_message: "hello".into(),
                turn_id: None,
            },
        );
        let json = serde_json::to_string(&submission).unwrap();
        let decoded: Submission = serde_json::from_str(&json).unwrap();
        assert_eq!(submission, decoded);
    }

    #[test]
    fn configure_op_serialization() {
        let op = Op::Configure {
            model: "mock-model".into(),
            system_prompt: Some("You are helpful.".into()),
        };
        let json = serde_json::to_string(&op).unwrap();
        assert!(json.contains("configure"));
        assert!(json.contains("mock-model"));
    }

    #[test]
    fn interrupt_is_unit_variant() {
        let op = Op::Interrupt;
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(json, r#"{"type":"interrupt"}"#);
    }
}
