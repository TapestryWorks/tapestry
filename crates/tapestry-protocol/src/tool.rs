//! Tool protocol: specs, calls, and results.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ids::ToolCallId;

/// Description of a tool exposed to the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema describing the arguments.
    pub parameters: Value,
}

/// A tool invocation requested by the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: ToolCallId,
    pub name: String,
    /// JSON arguments for the tool.
    ///
    /// Convention: every call should include a string `description` field
    /// stating why the tool is being invoked. This is a convention only; it
    /// is not validated here, and tools should not depend on it for execution.
    pub arguments: Value,
}

impl ToolCall {
    pub fn new(id: ToolCallId, name: impl Into<String>, arguments: Value) -> Self {
        Self {
            id,
            name: name.into(),
            arguments,
        }
    }
}

/// Outcome of a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    #[default]
    Completed,
    Failed,
    NotFound,
}

/// Result of one tool call, as stored in history and reported in events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    pub call_id: ToolCallId,
    pub tool_name: String,
    pub output: String,
    pub status: ToolCallStatus,
}

impl ToolResult {
    fn for_call(call: &ToolCall, output: String, status: ToolCallStatus) -> Self {
        Self {
            call_id: call.id.clone(),
            tool_name: call.name.clone(),
            output,
            status,
        }
    }

    pub fn completed(call: &ToolCall, output: impl Into<String>) -> Self {
        Self::for_call(call, output.into(), ToolCallStatus::Completed)
    }

    pub fn failed(call: &ToolCall, output: impl Into<String>) -> Self {
        Self::for_call(call, output.into(), ToolCallStatus::Failed)
    }

    pub fn not_found(call: &ToolCall) -> Self {
        Self::for_call(
            call,
            format!("tool `{}` not found", call.name),
            ToolCallStatus::NotFound,
        )
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn call() -> ToolCall {
        ToolCall::new("call_1".into(), "echo", json!({"input": "x"}))
    }

    #[test]
    fn tool_spec_round_trip() {
        let spec = ToolSpec {
            name: "echo".into(),
            description: "Echo the given input back.".into(),
            parameters: json!({"type": "object", "properties": {"input": {"type": "string"}}}),
        };
        let value = serde_json::to_value(&spec).unwrap();
        assert_eq!(value["name"], "echo");
        assert_eq!(value["parameters"]["type"], "object");
        assert_eq!(serde_json::from_value::<ToolSpec>(value).unwrap(), spec);
    }

    #[test]
    fn tool_call_keeps_the_given_id() {
        let a = ToolCall::new(ToolCallId::generate(), "echo", Value::Null);
        let b = ToolCall::new(ToolCallId::generate(), "echo", Value::Null);
        assert_ne!(a.id, b.id);
        assert_eq!(call().id.as_str(), "call_1");
    }

    #[test]
    fn result_constructors_copy_call_identity_and_set_status() {
        let c = call();
        let done = ToolResult::completed(&c, "x");
        assert_eq!(done.call_id, c.id);
        assert_eq!(done.tool_name, "echo");
        assert_eq!(done.status, ToolCallStatus::Completed);

        let failed = ToolResult::failed(&c, "boom");
        assert_eq!(failed.output, "boom");
        assert_eq!(failed.status, ToolCallStatus::Failed);

        let missing = ToolResult::not_found(&c);
        assert_eq!(missing.status, ToolCallStatus::NotFound);
        assert!(missing.output.contains("echo"));
    }

    #[test]
    fn result_round_trip_and_status_wire_format() {
        let result = ToolResult::not_found(&call());
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["status"], "not_found");
        assert_eq!(value["call_id"], "call_1");
        assert_eq!(serde_json::from_value::<ToolResult>(value).unwrap(), result);
        assert_eq!(
            serde_json::to_string(&ToolCallStatus::NotFound).unwrap(),
            r#""not_found""#
        );
    }
}
