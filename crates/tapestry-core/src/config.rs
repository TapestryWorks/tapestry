use tapestry_protocol::ThreadSettings;

/// Resolved configuration for a thread session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadConfig {
    pub model: String,
    pub system_prompt: Option<String>,
    pub max_tool_iterations: u32,
}

impl Default for ThreadConfig {
    fn default() -> Self {
        Self {
            model: "mock-model".into(),
            system_prompt: None,
            max_tool_iterations: 8,
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
        });
        assert_eq!(config.model, "gpt-test");
        assert_eq!(config.max_tool_iterations, 3);
        assert!(config.system_prompt.is_none());
    }
}
