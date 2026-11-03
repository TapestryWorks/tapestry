use tapestry_protocol::ThreadSettings;

/// Resolved configuration for a thread session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadConfig {
    pub model: String,
    pub system_prompt: Option<String>,
    pub max_tool_iterations: u32,
    /// Name of this agent, announced to the model in the system prompt.
    pub agent_name: Option<String>,
}

impl Default for ThreadConfig {
    fn default() -> Self {
        Self {
            model: "mock-model".into(),
            system_prompt: None,
            max_tool_iterations: 8,
            agent_name: None,
        }
    }
}

impl ThreadConfig {
    pub fn apply_settings(&mut self, settings: &ThreadSettings) {
        if let Some(model) = &settings.model {
            self.model = model.clone();
        }
        if let Some(prompt) = &settings.system_prompt {
            self.system_prompt = Some(prompt.clone());
        }
        if let Some(max_iters) = settings.max_tool_iterations {
            self.max_tool_iterations = max_iters;
        }
        if let Some(name) = &settings.agent_name {
            self.agent_name = Some(name.clone());
        }
    }

    /// System prompt sent to the model, with the agent identity appended if configured.
    pub fn effective_system_prompt(&self) -> Option<String> {
        match (&self.system_prompt, &self.agent_name) {
            (prompt, None) => prompt.clone(),
            (None, Some(name)) => Some(format!("Your name is {name}.")),
            (Some(prompt), Some(name)) => Some(format!("{prompt}\n\nYour name is {name}.")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_settings_merges_partial_updates() {
        let mut config = ThreadConfig::default();
        config.apply_settings(&ThreadSettings {
            model: Some("gpt-test".into()),
            system_prompt: None,
            max_tool_iterations: Some(3),
            agent_name: None,
        });
        assert_eq!(config.model, "gpt-test");
        assert_eq!(config.max_tool_iterations, 3);
        assert!(config.system_prompt.is_none());
        assert!(config.agent_name.is_none());
    }

    #[test]
    fn agent_name_updates_alone() {
        let mut config = ThreadConfig::default();
        config.apply_settings(&ThreadSettings {
            model: Some("m".into()),
            ..Default::default()
        });
        config.apply_settings(&ThreadSettings {
            agent_name: Some("planner".into()),
            ..Default::default()
        });
        assert_eq!(config.agent_name.as_deref(), Some("planner"));
        assert_eq!(config.model, "m");
    }

    #[test]
    fn system_prompt_without_agent_name_is_unchanged() {
        let mut config = ThreadConfig::default();
        assert_eq!(config.effective_system_prompt(), None);
        config.system_prompt = Some("Be brief.".into());
        assert_eq!(
            config.effective_system_prompt().as_deref(),
            Some("Be brief.")
        );
    }

    #[test]
    fn system_prompt_appends_agent_name() {
        let mut config = ThreadConfig {
            system_prompt: Some("Be brief.".into()),
            agent_name: Some("planner".into()),
            ..Default::default()
        };
        let prompt = config.effective_system_prompt().unwrap();
        assert!(prompt.starts_with("Be brief."));
        assert!(prompt.ends_with("Your name is planner."));

        config.system_prompt = None;
        assert_eq!(
            config.effective_system_prompt().as_deref(),
            Some("Your name is planner.")
        );
    }
}
