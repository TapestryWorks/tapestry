use super::{Agent, AgentDeps};

/// Default Agent implementation. ReAct scheduling is provided by
/// [`Agent::forward`] and uses the per-turn [`super::TurnContext`] as its
/// single source of iteration and budget state.
pub struct ReactAgent {
    deps: AgentDeps,
}

impl ReactAgent {
    /// Create a standard ReAct agent.
    ///
    /// The budget argument is retained for source compatibility; the runtime
    /// budget is read from the turn's `ThreadConfig`.
    pub fn new(deps: AgentDeps, _max_tool_iterations: u32) -> Self {
        Self { deps }
    }
}

impl Agent for ReactAgent {
    fn deps(&self) -> &AgentDeps {
        &self.deps
    }
}