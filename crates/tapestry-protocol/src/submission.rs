use serde::{Deserialize, Serialize};

use crate::common::{SubmissionId, TurnId};

/// Submission Queue entry — a request from the client to the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Submission {
    /// Correlates with [`crate::Event`] responses.
    pub id: SubmissionId,
    pub command: Command,
}

impl Submission {
    pub fn new(command: Command) -> Self {
        Self {
            id: SubmissionId::new(),
            command,
        }
    }

    pub fn with_id(id: SubmissionId, command: Command) -> Self {
        Self { id, command }
    }
}

/// Client-to-agent commands: intents that the thread may reject.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    /// Apply partial thread settings. The first one configures the thread.
    UpdateSettings { settings: ThreadSettings },
    /// Start processing user input for a turn.
    StartTurn {
        content: String,
        /// Speaker name for multi-agent scenarios.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<TurnId>,
    },
    /// Abort the in-flight turn without shutting down the thread.
    Interrupt,
}

/// Mutable thread/session settings applied via [`Command::UpdateSettings`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ThreadSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tool_iterations: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    /// Inject date/time-zone context into history; threads treat `None` as enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_time_context: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commands() -> [Command; 3] {
        [
            Command::UpdateSettings {
                settings: ThreadSettings {
                    model: Some("mock-model".into()),
                    system_prompt: Some("You are helpful.".into()),
                    ..Default::default()
                },
            },
            Command::StartTurn {
                content: "hello".into(),
                name: Some("alice".into()),
                turn_id: Some(TurnId::new()),
            },
            Command::Interrupt,
        ]
    }

    #[test]
    fn commands_round_trip_with_snake_case_tags() {
        let tags = ["update_settings", "start_turn", "interrupt"];
        for (command, tag) in commands().into_iter().zip(tags) {
            let submission = Submission::new(command);
            let value = serde_json::to_value(&submission).unwrap();
            assert_eq!(value["command"]["type"], tag);
            let decoded: Submission = serde_json::from_value(value).unwrap();
            assert_eq!(submission, decoded);
        }
    }

    #[test]
    fn submission_uses_command_field_not_op() {
        let value = serde_json::to_value(Submission::new(Command::Interrupt)).unwrap();
        assert!(value.get("id").is_some());
        assert_eq!(value["command"]["type"], "interrupt");
        assert!(value.get("op").is_none());
    }

    #[test]
    fn removed_commands_do_not_parse() {
        for json in [
            r#"{"type":"configure","model":"m"}"#,
            r#"{"type":"thread_settings","settings":{}}"#,
        ] {
            assert!(serde_json::from_str::<Command>(json).is_err());
            let submission = format!(r#"{{"id":"{}","command":{json}}}"#, SubmissionId::new());
            assert!(serde_json::from_str::<Submission>(&submission).is_err());
        }
    }

    #[test]
    fn start_turn_without_name_is_omitted_and_parses() {
        let command = Command::StartTurn {
            content: "hi".into(),
            name: None,
            turn_id: None,
        };
        let json = serde_json::to_string(&command).unwrap();
        assert!(!json.contains("name"));
        let decoded: Command =
            serde_json::from_str(r#"{"type":"start_turn","content":"hi"}"#).unwrap();
        assert_eq!(command, decoded);
    }

    #[test]
    fn interrupt_is_unit_variant() {
        let json = serde_json::to_string(&Command::Interrupt).unwrap();
        assert_eq!(json, r#"{"type":"interrupt"}"#);
    }

    #[test]
    fn thread_settings_agent_name_optional() {
        let s = ThreadSettings::default();
        assert_eq!(serde_json::to_string(&s).unwrap(), "{}");
        let s: ThreadSettings = serde_json::from_str(r#"{"agent_name":"planner"}"#).unwrap();
        assert_eq!(s.agent_name.as_deref(), Some("planner"));
        assert!(s.model.is_none());
    }

    #[test]
    fn thread_settings_include_time_context_roundtrip() {
        let s = ThreadSettings {
            include_time_context: Some(false),
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, r#"{"include_time_context":false}"#);
        let decoded: ThreadSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, decoded);
    }

    #[test]
    fn thread_settings_without_include_time_context_parses_to_none() {
        let s: ThreadSettings = serde_json::from_str(r#"{"model":"m"}"#).unwrap();
        assert!(s.include_time_context.is_none());
    }

    #[test]
    fn ids_are_reachable_through_common_path() {
        let id = crate::common::TurnId::new();
        let same: crate::TurnId = id;
        assert_eq!(id, same);
        let _ = crate::common::ids::ThreadId::new();
    }
}
