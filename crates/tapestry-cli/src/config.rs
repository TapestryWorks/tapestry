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

#[derive(Deserialize)]
struct AgentConfig {
    system_prompt: String,
}

pub(crate) fn load(
    cwd: &Path,
    home: Option<&Path>,
    agent_name: &str,
) -> Result<(GenaiProviderConfig, String)> {
    ensure!(
        !agent_name.is_empty()
            && agent_name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
        "agent name must contain only ASCII letters, digits, hyphens, or underscores"
    );

    let agent_file = Path::new("agents").join(format!("{agent_name}.yaml"));
    let (model, model_path): (ModelConfig, _) = read_yaml(cwd, home, Path::new("config.yaml"))?;
    let (agent, agent_path): (AgentConfig, _) = read_yaml(cwd, home, &agent_file)?;

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
    ensure!(
        !agent.system_prompt.trim().is_empty(),
        "{}: system_prompt is empty",
        agent_path.display()
    );

    let provider = GenaiProviderConfig::new(model.model.trim())
        .with_endpoint(endpoint.as_str())
        .with_api_key(model.api_key.trim());
    Ok((provider, agent.system_prompt))
}

fn read_yaml<T: DeserializeOwned>(
    cwd: &Path,
    home: Option<&Path>,
    relative: &Path,
) -> Result<(T, PathBuf)> {
    let paths: Vec<_> = std::iter::once(cwd)
        .chain(home.filter(|home| *home != cwd))
        .map(|root| root.join(".tapestry").join(relative))
        .collect();

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
    fn looks_up_each_file_in_cwd_then_home() {
        let cwd = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let local = cwd.path().join(".tapestry");
        let global = home.path().join(".tapestry");
        fs::create_dir_all(local.join("agents")).unwrap();
        fs::create_dir_all(global.join("agents")).unwrap();
        fs::write(
            global.join("config.yaml"),
            "base_url: https://example.com/v1\napi_key: test-key\nmodel: global",
        )
        .unwrap();
        fs::write(global.join("agents/reviewer.yaml"), "system_prompt: Global").unwrap();
        fs::write(local.join("agents/reviewer.yaml"), "system_prompt: Local").unwrap();

        let (model, prompt) = load(cwd.path(), Some(home.path()), "reviewer").unwrap();
        assert_eq!(model.model, "global");
        assert_eq!(prompt, "Local");

        fs::write(
            local.join("config.yaml"),
            "base_url: https://example.com/v1\napi_key: [sensitive-test-value]\nmodel: local",
        )
        .unwrap();
        let error = load(cwd.path(), Some(home.path()), "reviewer")
            .unwrap_err()
            .to_string();
        assert!(error.contains(&local.join("config.yaml").display().to_string()));
        assert!(!error.contains("sensitive-test-value"));
    }

    #[test]
    fn rejects_agent_names_that_can_escape_the_agents_directory() {
        let error = load(Path::new("."), None, "../reviewer").unwrap_err();
        assert!(error.to_string().contains("agent name"));
    }
}
