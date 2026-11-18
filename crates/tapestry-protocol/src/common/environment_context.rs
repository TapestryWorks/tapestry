//! Environment context shared by events and model requests.

use serde::{Deserialize, Serialize};

/// Complete snapshot of the date and time zone told to the model.
///
/// The last one in history is the baseline for deciding whether to send another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum EnvironmentContext {
    Available { date: String, timezone: String },
    Unavailable,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn context_round_trip() {
        let context = EnvironmentContext::Available {
            date: "2026-11-05".into(),
            timezone: "UTC".into(),
        };
        let text = serde_json::to_string(&context).unwrap();
        assert_eq!(
            serde_json::from_str::<EnvironmentContext>(&text).unwrap(),
            context
        );
        assert_eq!(
            serde_json::to_value(EnvironmentContext::Unavailable).unwrap(),
            json!({"state": "unavailable"})
        );
    }
}
