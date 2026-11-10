use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            pub fn from_uuid(uuid: Uuid) -> Self {
                Self(uuid)
            }

            pub fn as_uuid(&self) -> &Uuid {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(Uuid::parse_str(s)?))
            }
        }
    };
}

id_type!(ThreadId);
id_type!(TurnId);
id_type!(SubmissionId);

/// Opaque tool call identifier, normally issued by the model provider.
///
/// Never parse it: its format is provider-defined.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolCallId(String);

impl ToolCallId {
    /// Fallback for providers that give no id; `call_` plus a simple UUID.
    pub fn generate() -> Self {
        Self(format!("call_{}", Uuid::new_v4().simple()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for ToolCallId {
    fn from(id: String) -> Self {
        Self(id)
    }
}

impl From<&str> for ToolCallId {
    fn from(id: &str) -> Self {
        Self(id.to_string())
    }
}

impl fmt::Display for ToolCallId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_id_roundtrip() {
        let id = ThreadId::new();
        let parsed: ThreadId = id.to_string().parse().expect("parse");
        assert_eq!(id, parsed);
    }

    #[test]
    fn serde_roundtrip() {
        let id = TurnId::new();
        let json = serde_json::to_string(&id).unwrap();
        let decoded: TurnId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, decoded);
    }

    #[test]
    fn tool_call_id_serializes_transparently() {
        let id = ToolCallId::from("call_abc");
        assert_eq!(serde_json::to_string(&id).unwrap(), r#""call_abc""#);
        assert_eq!(
            serde_json::from_str::<ToolCallId>(r#""call_abc""#).unwrap(),
            id
        );
        assert_eq!(id.as_str(), "call_abc");
        assert_eq!(id.to_string(), "call_abc");
    }

    #[test]
    fn generated_tool_call_ids_are_prefixed_and_distinct() {
        let a = ToolCallId::generate();
        let b = ToolCallId::generate();
        assert!(a.as_str().starts_with("call_") && a.as_str().len() > "call_".len());
        assert_ne!(a, b);
    }
}
