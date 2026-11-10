use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tapestry_protocol::{ToolCall, ToolResult, ToolSpec};

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn spec(&self) -> ToolSpec;
    /// Always returns a result; failures must not be surfaced as errors.
    /// Build it with `ToolResult::completed/failed(call, ..)`; the agent loop
    /// overwrites `call_id` and `tool_name` with those of `call`.
    async fn execute(&self, call: &ToolCall) -> ToolResult;
}

/// Registry of tools available to the agent loop.
#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// Specs of all registered tools, sorted by name for deterministic requests.
    /// Each schema is extended with a required `description` argument that
    /// states why the model is calling the tool.
    pub fn specs(&self) -> Vec<ToolSpec> {
        let mut specs: Vec<ToolSpec> = self
            .tools
            .values()
            .map(|tool| {
                let mut spec = tool.spec();
                require_call_description(&mut spec.parameters);
                spec
            })
            .collect();
        specs.sort_by(|a, b| a.name.cmp(&b.name));
        specs
    }
}

/// Adds a required string `description` property to an object schema.
/// A tool-defined `description` property is kept; non-object schemas are untouched.
fn require_call_description(schema: &mut Value) {
    let Some(obj) = schema.as_object_mut() else {
        return;
    };
    let properties = obj
        .entry("properties")
        .or_insert_with(|| Value::Object(Default::default()));
    if let Some(properties) = properties.as_object_mut() {
        properties.entry("description").or_insert_with(|| {
            serde_json::json!({
                "type": "string",
                "description": "Why this tool is being called.",
            })
        });
    }
    let required = obj
        .entry("required")
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Some(required) = required.as_array_mut() {
        if !required.iter().any(|v| v == "description") {
            required.push(Value::String("description".into()));
        }
    }
}

/// Simple echo tool for mock demonstrations.
pub struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "echo".into(),
            description: "Echo the given input back.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"input": {"type": "string"}},
                "required": ["input"],
            }),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let input = call
            .arguments
            .get("input")
            .and_then(Value::as_str)
            .unwrap_or("");
        ToolResult::completed(call, input)
    }
}

#[cfg(test)]
mod tests {
    use tapestry_protocol::{ToolCallId, ToolCallStatus};

    use super::*;

    fn call(name: &str, arguments: Value) -> ToolCall {
        ToolCall::new(ToolCallId::generate(), name, arguments)
    }

    struct NamedTool(&'static str);

    #[async_trait]
    impl Tool for NamedTool {
        fn name(&self) -> &str {
            self.0
        }
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: self.0.into(),
                description: String::new(),
                parameters: serde_json::json!({"type": "object"}),
            }
        }
        async fn execute(&self, call: &ToolCall) -> ToolResult {
            ToolResult::failed(call, "boom")
        }
    }

    #[tokio::test]
    async fn echo_tool_returns_input() {
        let call = call("echo", serde_json::json!({"input": "ping"}));
        let result = EchoTool.execute(&call).await;
        assert_eq!(result.output, "ping");
        assert_eq!(result.status, ToolCallStatus::Completed);
        assert_eq!(result.call_id, call.id);
        assert_eq!(result.tool_name, "echo");
    }

    #[tokio::test]
    async fn failure_is_a_result_not_an_error() {
        let result = NamedTool("x").execute(&call("x", Value::Null)).await;
        assert_eq!(result.status, ToolCallStatus::Failed);
        assert_eq!(result.output, "boom");
    }

    #[test]
    fn echo_spec_describes_input() {
        let spec = EchoTool.spec();
        assert_eq!(spec.name, "echo");
        assert_eq!(spec.parameters["properties"]["input"]["type"], "string");
    }

    #[test]
    fn registry_lookup() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        assert!(registry.get("echo").is_some());
        assert!(registry.get("missing").is_none());
    }

    #[test]
    fn registry_specs_require_description() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        registry.register(Arc::new(NamedTool("bare")));
        for spec in registry.specs() {
            assert_eq!(
                spec.parameters["properties"]["description"]["type"],
                "string"
            );
            let required = spec.parameters["required"].as_array().unwrap();
            assert_eq!(required.iter().filter(|v| *v == "description").count(), 1);
        }
        let echo = registry
            .specs()
            .into_iter()
            .find(|s| s.name == "echo")
            .unwrap();
        assert_eq!(
            echo.parameters["required"],
            serde_json::json!(["input", "description"])
        );
        // The tool's own spec is unchanged.
        assert!(EchoTool.spec().parameters["properties"]
            .get("description")
            .is_none());
    }

    #[test]
    fn require_description_skips_non_object_and_keeps_custom() {
        let mut schema = serde_json::json!(true);
        require_call_description(&mut schema);
        assert_eq!(schema, serde_json::json!(true));

        let mut schema = serde_json::json!({
            "type": "object",
            "properties": {"description": {"type": "string", "description": "custom"}},
            "required": ["description"],
        });
        require_call_description(&mut schema);
        assert_eq!(schema["properties"]["description"]["description"], "custom");
        assert_eq!(schema["required"], serde_json::json!(["description"]));
    }

    #[test]
    fn registry_specs_are_sorted() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(NamedTool("zeta")));
        registry.register(Arc::new(EchoTool));
        let names: Vec<_> = registry.specs().into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["echo", "zeta"]);
    }
}
