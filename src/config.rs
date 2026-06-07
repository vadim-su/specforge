use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const DEFAULT_SPEC: &str = "spec.adoc";
pub const CONFIG_FILE: &str = ".specforge/config.yaml";
pub const STATE_DIR: &str = ".specforge/state";
pub const TASKS_DIR: &str = ".specforge/tasks";
pub const CURRENT_SPEC: &str = ".specforge/state/current.adoc";
pub const CURRENT_MODEL: &str = ".specforge/state/current.model.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectConfig {
    #[serde(default)]
    pub checks: Vec<ProjectCheckConfig>,
    #[serde(default)]
    pub file_access: ProjectFileAccessConfig,
    #[serde(default)]
    pub integrations: ProjectIntegrationsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectCheckConfig {
    pub command: Vec<String>,
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectFileAccessConfig {
    #[serde(default)]
    pub allowed: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectIntegrationsConfig {
    #[serde(default)]
    pub mcp: BTreeMap<String, ProjectMcpServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectMcpServerConfig {
    pub command: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env_vars: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

pub fn load_project_config() -> Result<ProjectConfig> {
    let path = Path::new(CONFIG_FILE);
    if !path.exists() {
        return Ok(ProjectConfig::default());
    }

    let source =
        fs::read_to_string(path).with_context(|| format!("failed to read {CONFIG_FILE}"))?;
    parse_project_config(&source).with_context(|| format!("failed to parse {CONFIG_FILE}"))
}

pub fn write_project_config(config: &ProjectConfig) -> Result<()> {
    let path = Path::new(CONFIG_FILE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let source = serde_yaml_ng::to_string(config).context("failed to serialize project config")?;
    fs::write(path, source).with_context(|| format!("failed to write {CONFIG_FILE}"))
}

pub fn clear_project_config() -> Result<()> {
    let path = Path::new(CONFIG_FILE);
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("failed to remove {CONFIG_FILE}"))?;
    }

    Ok(())
}

pub fn project_config_path() -> PathBuf {
    PathBuf::from(CONFIG_FILE)
}

fn parse_project_config(source: &str) -> Result<ProjectConfig> {
    serde_yaml_ng::from_str(source).context("invalid project config yaml")
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_check_config() {
        let config = parse_project_config(
            r#"
checks:
  - command: ["cargo", "test", "--color", "never"]
    timeout_seconds: 120
"#,
        )
        .expect("config should parse");

        assert_eq!(
            config.checks,
            vec![ProjectCheckConfig {
                command: vec![
                    "cargo".to_string(),
                    "test".to_string(),
                    "--color".to_string(),
                    "never".to_string()
                ],
                timeout_seconds: 120,
            }]
        );
    }

    #[test]
    fn parses_file_access_config() {
        let config = parse_project_config(
            r#"
file_access:
  allowed:
    - Cargo.toml
    - src/
"#,
        )
        .expect("config should parse");

        assert_eq!(
            config.file_access,
            ProjectFileAccessConfig {
                allowed: vec!["Cargo.toml".to_string(), "src/".to_string()],
            }
        );
    }

    #[test]
    fn parses_mcp_integration_config() {
        let config = parse_project_config(
            r#"
integrations:
  mcp:
    context7:
      command: "npx"
      args: ["-y", "@upstash/context7-mcp"]
      env_vars: ["LOCAL_TOKEN"]
      env:
        MY_ENV_VAR: "MY_ENV_VALUE"
    figma:
      command: "figma-mcp"
      enabled: false
"#,
        )
        .expect("config should parse");

        let context7 = config
            .integrations
            .mcp
            .get("context7")
            .expect("context7 config should parse");
        assert_eq!(context7.command, "npx");
        assert_eq!(context7.args, vec!["-y", "@upstash/context7-mcp"]);
        assert_eq!(context7.env_vars, vec!["LOCAL_TOKEN"]);
        assert_eq!(
            context7.env.get("MY_ENV_VAR").map(String::as_str),
            Some("MY_ENV_VALUE")
        );

        let figma = config
            .integrations
            .mcp
            .get("figma")
            .expect("figma config should parse");
        assert!(!figma.enabled);
        assert!(figma.args.is_empty());
    }
}
