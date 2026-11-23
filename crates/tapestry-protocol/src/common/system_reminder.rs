//! Instructions recorded as events and delivered with the next model request.

use serde::{Deserialize, Serialize};

/// Instruction recorded as a `SystemReminder` event; the next completed model
/// request consumes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reminder", rename_all = "snake_case")]
pub enum SystemReminder {
    /// The tool iteration limit was reached; answer without tools.
    ToolLimitReached,
    /// Caller-defined reminder text, sent to the model verbatim.
    Custom { text: String },
}

impl SystemReminder {
    pub fn custom(text: impl Into<String>) -> Self {
        Self::Custom { text: text.into() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reminder_round_trip() {
        for reminder in [
            SystemReminder::ToolLimitReached,
            SystemReminder::custom("x"),
        ] {
            let text = serde_json::to_string(&reminder).unwrap();
            assert_eq!(
                serde_json::from_str::<SystemReminder>(&text).unwrap(),
                reminder
            );
        }
    }
}
