use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use serde::{de::DeserializeOwned, Deserialize};
use tapestry_core::GenaiProviderConfig;
use url::Url;

#[derive(Deserialize)]
struct ModelConfig {
    base_url: String,
    api_key: String,
    model: String,
}

/// System prompt used when no agent is selected.
pub(crate) const DEFAULT_SYSTEM_PROMPT: &str = "You are a helpful assistant.";

#[derive(Deserialize)]
struct AgentConfig {
    system_prompt: String,
}

pub(crate) fn load(
    cwd: &Path,
    home: Option<&Path>,
    agent_name: Option<&str>,
) -> Result<(GenaiProviderConfig, String)> {
    if let Some(agent_name) = agent_name {
        validate_agent_name(agent_name)?;
    }
    let (model, model_path): (ModelConfig, _) = read_yaml(cwd, home, Path::new("config.yaml"))?;
    let provider = provider_config(model, &model_path)?;
    let system_prompt = match agent_name {
        Some(agent_name) => load_agent(cwd, home, agent_name)?,
        None => DEFAULT_SYSTEM_PROMPT.to_owned(),
    };
    Ok((provider, system_prompt))
}

fn validate_agent_name(agent_name: &str) -> Result<()> {
    ensure!(
        !agent_name.is_empty()
            && agent_name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
        "agent name must contain only ASCII letters, digits, hyphens, or underscores"
    );
    Ok(())
}

fn load_agent(cwd: &Path, home: Option<&Path>, agent_name: &str) -> Result<String> {
    let agent_file = Path::new("agents").join(format!("{agent_name}.yaml"));
    let (agent, agent_path): (AgentConfig, _) = read_yaml(cwd, home, &agent_file)?;
    ensure!(
        !agent.system_prompt.trim().is_empty(),
        "{}: system_prompt is empty",
        agent_path.display()
    );
    Ok(agent.system_prompt)
}

fn provider_config(model: ModelConfig, model_path: &Path) -> Result<GenaiProviderConfig> {
    let endpoint = Url::parse(model.base_url.trim()).ok().filter(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.has_host()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
    });
    let Some(endpoint) = endpoint else {
        bail!(
            "{}: base_url must be an HTTP(S) URL without credentials, a query, or a fragment",
            model_path.display()
        );
    };

    ensure!(
        !model.model.trim().is_empty(),
        "{}: model is empty",
        model_path.display()
    );
    ensure!(
        !model.api_key.trim().is_empty(),
        "{}: api_key is empty",
        model_path.display()
    );
    Ok(GenaiProviderConfig::new(model.model.trim())
        .with_endpoint(endpoint.as_str())
        .with_api_key(model.api_key.trim()))
}

fn read_yaml<T: DeserializeOwned>(
    cwd: &Path,
    home: Option<&Path>,
    relative: &Path,
) -> Result<(T, PathBuf)> {
    let agents_relative = relative.strip_prefix("agents").unwrap_or(relative);
    let mut paths = Vec::new();
    for root in std::iter::once(cwd).chain(home.filter(|home| *home != cwd)) {
        paths.push(root.join(".tapestry").join(relative));
        paths.push(root.join(".agents").join(agents_relative));
    }

    for path in &paths {
        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
        };
        // YAML parser diagnostics can contain field values, including the API key.
        let config = serde_yaml_ng::from_str(&contents).map_err(|error| {
            let location = error
                .location()
                .map(|at| format!(" at line {}, column {}", at.line(), at.column()))
                .unwrap_or_default();
            anyhow::anyhow!("invalid YAML configuration in {}{location}", path.display())
        })?;
        return Ok((config, path.clone()));
    }
    let searched = paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    bail!("configuration file not found; searched {searched}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_up_files_in_tapestry_then_agents_then_home() {
        let cwd = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let local_tapestry = cwd.path().join(".tapestry");
        let local_agents = cwd.path().join(".agents");
        let home_tapestry = home.path().join(".tapestry");
        let home_agents = home.path().join(".agents");
        for dir in [
            local_tapestry.join("agents"),
            local_agents.clone(),
            home_tapestry.join("agents"),
            home_agents.clone(),
        ] {
            fs::create_dir_all(dir).unwrap();
        }
        let model_config = |model: &str| {
            format!("base_url: https://example.com/v1\napi_key: test-key\nmodel: {model}")
        };
        fs::write(
            local_tapestry.join("config.yaml"),
            model_config("local-tapestry"),
        )
        .unwrap();
        fs::write(
            local_agents.join("config.yaml"),
            model_config("local-agents"),
        )
        .unwrap();
        fs::write(
            home_tapestry.join("config.yaml"),
            model_config("home-tapestry"),
        )
        .unwrap();
        fs::write(home_agents.join("config.yaml"), model_config("home-agents")).unwrap();
        fs::write(
            local_agents.join("reviewer.yaml"),
            "system_prompt: Local agents",
        )
        .unwrap();
        fs::write(
            home_tapestry.join("agents/reviewer.yaml"),
            "system_prompt: Home tapestry",
        )
        .unwrap();
        fs::write(
            home_agents.join("reviewer.yaml"),
            "system_prompt: Home agents",
        )
        .unwrap();

        let (model, prompt) = load(cwd.path(), Some(home.path()), Some("reviewer")).unwrap();
        assert_eq!(model.model, "local-tapestry");
        assert_eq!(prompt, "Local agents");

        fs::remove_file(local_tapestry.join("config.yaml")).unwrap();
        fs::write(
            local_tapestry.join("agents/reviewer.yaml"),
            "system_prompt: Local tapestry",
        )
        .unwrap();
        let (model, prompt) = load(cwd.path(), Some(home.path()), Some("reviewer")).unwrap();
        assert_eq!(model.model, "local-agents");
        assert_eq!(prompt, "Local tapestry");

        fs::remove_file(local_agents.join("config.yaml")).unwrap();
        fs::remove_file(local_tapestry.join("agents/reviewer.yaml")).unwrap();
        fs::remove_file(local_agents.join("reviewer.yaml")).unwrap();
        let (model, prompt) = load(cwd.path(), Some(home.path()), Some("reviewer")).unwrap();
        assert_eq!(model.model, "home-tapestry");
        assert_eq!(prompt, "Home tapestry");

        fs::remove_file(home_tapestry.join("config.yaml")).unwrap();
        fs::remove_file(home_tapestry.join("agents/reviewer.yaml")).unwrap();
        let (model, prompt) = load(cwd.path(), Some(home.path()), Some("reviewer")).unwrap();
        assert_eq!(model.model, "home-agents");
        assert_eq!(prompt, "Home agents");

        fs::write(
            home_agents.join("config.yaml"),
            "base_url: https://example.com/v1\napi_key: [sensitive-test-value]\nmodel: local",
        )
        .unwrap();
        let error = load(cwd.path(), Some(home.path()), Some("reviewer"))
            .unwrap_err()
            .to_string();
        assert!(error.contains(&home_agents.join("config.yaml").display().to_string()));
        assert!(!error.contains("sensitive-test-value"));
    }

    #[test]
    fn uses_the_default_system_prompt_without_an_agent() {
        let cwd = tempfile::tempdir().unwrap();
        let tapestry = cwd.path().join(".tapestry");
        fs::create_dir_all(&tapestry).unwrap();
        fs::write(
            tapestry.join("config.yaml"),
            "base_url: https://example.com/v1\napi_key: test-key\nmodel: m",
        )
        .unwrap();

        let (model, prompt) = load(cwd.path(), None, None).unwrap();
        assert_eq!(model.model, "m");
        assert_eq!(prompt, DEFAULT_SYSTEM_PROMPT);
        assert_eq!(prompt, "You are a helpful assistant.");

        let error = load(cwd.path(), None, Some("missing")).unwrap_err();
        assert!(error.to_string().contains("configuration file not found"));
    }

    #[test]
    fn requires_model_config_without_an_agent() {
        let cwd = tempfile::tempdir().unwrap();
        let error = load(cwd.path(), None, None).unwrap_err();
        assert!(error.to_string().contains("config.yaml"));
    }

    #[test]
    fn rejects_agent_names_that_can_escape_the_agents_directory() {
        let error = load(Path::new("."), None, Some("../reviewer")).unwrap_err();
        assert!(error.to_string().contains("agent name"));
    }
}
