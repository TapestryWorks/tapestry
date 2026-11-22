use std::sync::Arc;

use tapestry_protocol::{FinishReason, SystemReminder, ToolCall, ToolResult};

use crate::config::ThreadConfig;
use crate::error::CoreError;

/// What the turn driver should do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnStep {
    CallModel {
        allow_tools: bool,
        reminder: Option<SystemReminder>,
    },
    Done {
        finish: FinishReason,
        last_agent_message: Option<String>,
    },
}

/// Synchronous policy for deciding how a single turn progresses.
pub trait Agent: Send {
    fn next_step(&self) -> TurnStep;

    /// Return the tool calls to execute as an unchanged, ordered subsequence.
    fn on_model_output(
        &mut self,
        content: Option<&str>,
        tool_calls: &[ToolCall],
    ) -> Result<Vec<ToolCall>, CoreError>;

    fn on_tool_results(&mut self, results: &[ToolResult]) -> Result<(), CoreError>;
}

/// Creates fresh per-turn agent policies from the current thread configuration.
pub type AgentFactory = Arc<dyn Fn(&ThreadConfig) -> Box<dyn Agent> + Send + Sync>;
