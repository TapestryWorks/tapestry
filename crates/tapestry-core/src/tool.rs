use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

/// Description of a tool exposed to the model.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema describing the arguments.
    pub parameters: Value,
}

/// Result of executing a tool. Failures are expressed with `success: false`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub output: String,
    pub success: bool,
}

impl ToolResult {
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            success: true,
        }
    }

    pub fn failure(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            success: false,
        }
    }
}

/// Execution context available to tools during a turn.
#[derive(Debug, Clone, Default)]
pub struct ToolContext {
    pub turn_user_message: String,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn spec(&self) -> ToolSpec;
    /// Always returns a result; failures must not be surfaced as errors.
    async fn execute(&self, arguments: Value, ctx: &ToolContext) -> ToolResult;
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
    pub fn specs(&self) -> Vec<ToolSpec> {
        let mut specs: Vec<ToolSpec> = self.tools.values().map(|tool| tool.spec()).collect();
        specs.sort_by(|a, b| a.name.cmp(&b.name));
        specs
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

    async fn execute(&self, arguments: Value, _ctx: &ToolContext) -> ToolResult {
        let input = arguments
            .get("input")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        ToolResult::success(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        async fn execute(&self, _arguments: Value, _ctx: &ToolContext) -> ToolResult {
            ToolResult::failure("boom")
        }
    }

    #[tokio::test]
    async fn echo_tool_returns_input() {
        let result = EchoTool
            .execute(
                serde_json::json!({"input": "ping"}),
                &ToolContext::default(),
            )
            .await;
        assert_eq!(result.output, "ping");
        assert!(result.success);
    }

    #[tokio::test]
    async fn failure_is_a_result_not_an_error() {
        let result = NamedTool("x")
            .execute(Value::Null, &ToolContext::default())
            .await;
        assert!(!result.success);
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
    fn registry_specs_are_sorted() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(NamedTool("zeta")));
        registry.register(Arc::new(EchoTool));
        let names: Vec<_> = registry.specs().into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["echo", "zeta"]);
    }
}
