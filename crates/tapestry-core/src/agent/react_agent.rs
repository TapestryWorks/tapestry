use super::{Agent, AgentBindings};

/// Default Agent implementation. ReAct scheduling is provided by
/// [`Agent::forward`] and uses the per-turn [`super::TurnContext`] as its
/// single source of iteration and budget state.
pub struct ReactAgent {
    bindings: AgentBindings,
}

impl ReactAgent {
    /// Create a standard ReAct agent.
    ///
    /// The budget argument is retained for source compatibility; the runtime
    /// budget is read from the turn's `ThreadConfig`.
    pub fn new(bindings: AgentBindings, _max_tool_iterations: u32) -> Self {
        Self { bindings }
    }
}

impl Agent for ReactAgent {
    fn bindings(&self) -> &AgentBindings {
        &self.bindings
    }

    fn bindings_mut(&mut self) -> &mut AgentBindings {
        &mut self.bindings
    }
}
