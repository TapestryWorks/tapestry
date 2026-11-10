//! Small value types shared by every protocol layer.

use std::ops::AddAssign;

use serde::{Deserialize, Serialize};

/// Why a turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    #[default]
    Completed,
    ExceedMaxIters,
}

/// Token consumption of one or more model calls.
///
/// `input_tokens` excludes cached tokens; `output_tokens` includes reasoning.
/// `total_tokens` is derived: it is written on serialization and ignored on
/// deserialization, so it can never disagree with the four counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(into = "TokenUsageWire", from = "TokenUsageWire")]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

impl TokenUsage {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_read_tokens)
            .saturating_add(self.cache_write_tokens)
    }
}

impl AddAssign for TokenUsage {
    fn add_assign(&mut self, rhs: Self) {
        self.input_tokens = self.input_tokens.saturating_add(rhs.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(rhs.output_tokens);
        self.cache_read_tokens = self.cache_read_tokens.saturating_add(rhs.cache_read_tokens);
        self.cache_write_tokens = self
            .cache_write_tokens
            .saturating_add(rhs.cache_write_tokens);
    }
}

#[derive(Serialize, Deserialize)]
struct TokenUsageWire {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_tokens: u64,
    #[serde(default)]
    cache_write_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}

impl From<TokenUsage> for TokenUsageWire {
    fn from(usage: TokenUsage) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            cache_write_tokens: usage.cache_write_tokens,
            total_tokens: usage.total_tokens(),
        }
    }
}

impl From<TokenUsageWire> for TokenUsage {
    fn from(wire: TokenUsageWire) -> Self {
        Self {
            input_tokens: wire.input_tokens,
            output_tokens: wire.output_tokens,
            cache_read_tokens: wire.cache_read_tokens,
            cache_write_tokens: wire.cache_write_tokens,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn usage() -> TokenUsage {
        TokenUsage {
            input_tokens: 10,
            output_tokens: 5,
            cache_read_tokens: 3,
            cache_write_tokens: 2,
        }
    }

    #[test]
    fn serialization_includes_derived_total() {
        assert_eq!(
            serde_json::to_value(usage()).unwrap(),
            json!({
                "input_tokens": 10,
                "output_tokens": 5,
                "cache_read_tokens": 3,
                "cache_write_tokens": 2,
                "total_tokens": 20
            })
        );
    }

    #[test]
    fn inconsistent_total_is_ignored_on_deserialization() {
        let decoded: TokenUsage = serde_json::from_value(json!({
            "input_tokens": 10,
            "output_tokens": 5,
            "cache_read_tokens": 3,
            "cache_write_tokens": 2,
            "total_tokens": 999
        }))
        .unwrap();
        assert_eq!(decoded, usage());
        assert_eq!(decoded.total_tokens(), 20);
    }

    #[test]
    fn missing_counters_default_to_zero() {
        let decoded: TokenUsage = serde_json::from_value(json!({"input_tokens": 4})).unwrap();
        assert_eq!(
            decoded,
            TokenUsage {
                input_tokens: 4,
                ..Default::default()
            }
        );
    }

    #[test]
    fn add_assign_sums_each_counter_and_saturates() {
        let mut total = usage();
        total += usage();
        assert_eq!(total.input_tokens, 20);
        assert_eq!(total.cache_write_tokens, 4);
        assert_eq!(total.total_tokens(), 40);

        let mut max = TokenUsage {
            input_tokens: u64::MAX,
            ..Default::default()
        };
        max += usage();
        assert_eq!(max.input_tokens, u64::MAX);
        assert_eq!(max.total_tokens(), u64::MAX);
    }

    #[test]
    fn finish_reason_wire_format() {
        assert_eq!(
            serde_json::to_string(&FinishReason::ExceedMaxIters).unwrap(),
            r#""exceed_max_iters""#
        );
    }
}
