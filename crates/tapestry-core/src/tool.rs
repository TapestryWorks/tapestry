use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::CoreError;

/// Result of executing a tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub output: String,
    pub success: bool,
}

/// Execution context available to tools during a turn.
#[derive(Debug, Clone, Default)]
pub struct ToolContext {
    pub turn_user_message: String,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    async fn execute(&self, arguments: Value, ctx: &ToolContext) -> Result<ToolResult, CoreError>;
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
}

/// Simple echo tool for mock demonstrations.
pub struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }

    async fn execute(&self, arguments: Value, _ctx: &ToolContext) -> Result<ToolResult, CoreError> {
        let input = arguments
            .get("input")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        Ok(ToolResult {
            output: input,
            success: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn echo_tool_returns_input() {
        let tool = EchoTool;
        let result = tool
            .execute(
                serde_json::json!({"input": "ping"}),
                &ToolContext::default(),
            )
            .await
            .unwrap();
        assert_eq!(result.output, "ping");
        assert!(result.success);
    }

    #[test]
    fn registry_lookup() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        assert!(registry.get("echo").is_some());
        assert!(registry.get("missing").is_none());
    }
}
