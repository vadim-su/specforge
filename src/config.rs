use std::{
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectCheckConfig {
    pub command: Vec<String>,
    pub timeout_seconds: u64,
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
}
