use genai::adapter::AdapterKind;
use genai::resolver::{AuthData, Endpoint};
use genai::{Client, ServiceTarget};

use crate::error::CoreError;

/// Parsed model identifier supporting simple names and `namespace::model` form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedModelName {
    /// Provider namespace when using `namespace::model` syntax.
    pub namespace: Option<String>,
    /// Bare model name without namespace prefix.
    pub name: String,
    /// Original model string passed by the caller.
    pub full: String,
}

/// Explicit configuration for a genai-backed provider.
///
/// When fields are unset, genai falls back to its built-in adapter defaults and
/// standard environment variables (for example `OPENAI_API_KEY`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GenaiProviderConfig {
    /// Model name (`gpt-4o-mini`) or namespaced form (`ollama::llama3`).
    pub model: String,
    /// Inline API key. Never commit this value; prefer environment variables.
    pub api_key: Option<String>,
    /// Environment variable holding the API key (for example `OPENAI_API_KEY`).
    pub api_key_env: Option<String>,
    /// Optional base URL override for the resolved adapter.
    pub endpoint: Option<String>,
    /// Bind the client to a single adapter (for example `AdapterKind::Ollama`).
    pub adapter: Option<AdapterKind>,
}

impl GenaiProviderConfig {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            ..Self::default()
        }
    }

    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    pub fn with_api_key_env(mut self, env: impl Into<String>) -> Self {
        self.api_key_env = Some(env.into());
        self
    }

    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }

    pub fn with_adapter(mut self, adapter: AdapterKind) -> Self {
        self.adapter = Some(adapter);
        self
    }
}

/// Split a model string into optional namespace and bare model name.
pub fn parse_model_name(model: &str) -> ParsedModelName {
    let trimmed = model.trim();
    if let Some((namespace, name)) = trimmed.split_once("::") {
        let namespace = namespace.trim();
        let name = name.trim();
        if !namespace.is_empty() && !name.is_empty() {
            return ParsedModelName {
                namespace: Some(namespace.to_string()),
                name: name.to_string(),
                full: trimmed.to_string(),
            };
        }
    }

    ParsedModelName {
        namespace: None,
        name: trimmed.to_string(),
        full: trimmed.to_string(),
    }
}

/// Return the default API-key environment variable for a model, when known.
pub fn default_api_key_env(model: &str) -> Option<&'static str> {
    let parsed = parse_model_name(model);
    let adapter = parsed
        .namespace
        .as_deref()
        .and_then(AdapterKind::from_lower_str)
        .or_else(|| AdapterKind::from_model(&parsed.name).ok());

    adapter.and_then(|kind| kind.default_key_env_name())
}

/// Build a configured [`Client`] from explicit settings and genai defaults.
pub fn build_client(config: &GenaiProviderConfig) -> Result<Client, CoreError> {
    let mut builder = Client::builder();

    if let Some(adapter) = config.adapter {
        builder = builder.with_adapter_kind(adapter);
    }

    if config.api_key.is_some() || config.api_key_env.is_some() {
        let inline_key = config.api_key.clone();
        let env_name = config.api_key_env.clone();
        builder = builder.with_auth_resolver_fn(move |_model_iden| {
            if let Some(key) = &inline_key {
                Ok(Some(AuthData::from_single(key.clone())))
            } else if let Some(env) = &env_name {
                Ok(Some(AuthData::from_env(env.clone())))
            } else {
                Ok(None)
            }
        });
    }

    if let Some(endpoint) = &config.endpoint {
        let endpoint = Endpoint::from_owned(endpoint.clone());
        builder = builder.with_service_target_resolver_fn(move |mut target: ServiceTarget| {
            target.endpoint = endpoint.clone();
            Ok(target)
        });
    }

    Ok(builder.build())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_model_name() {
        let parsed = parse_model_name("gpt-4o-mini");
        assert_eq!(parsed.namespace, None);
        assert_eq!(parsed.name, "gpt-4o-mini");
        assert_eq!(parsed.full, "gpt-4o-mini");
    }

    #[test]
    fn parse_namespaced_model_name() {
        let parsed = parse_model_name("ollama::llama3.2");
        assert_eq!(parsed.namespace.as_deref(), Some("ollama"));
        assert_eq!(parsed.name, "llama3.2");
        assert_eq!(parsed.full, "ollama::llama3.2");
    }

    #[test]
    fn parse_trims_whitespace() {
        let parsed = parse_model_name("  openai::gpt-4o  ");
        assert_eq!(parsed.namespace.as_deref(), Some("openai"));
        assert_eq!(parsed.name, "gpt-4o");
    }

    #[test]
    fn default_api_key_env_for_openai_model() {
        assert_eq!(default_api_key_env("gpt-4o-mini"), Some("OPENAI_API_KEY"));
    }

    #[test]
    fn default_api_key_env_for_namespaced_model() {
        assert_eq!(
            default_api_key_env("groq::llama-3.3-70b-versatile"),
            Some("GROQ_API_KEY")
        );
    }

    #[test]
    fn default_api_key_env_for_ollama_is_none() {
        assert_eq!(default_api_key_env("ollama::llama3"), None);
    }
}
