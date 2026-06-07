use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

use crate::{
    config::{ProjectMcpServerConfig, load_project_config},
    profiles::{TechProfile, detect_profiles, render_profiles_prompt},
};

const MAX_PROJECT_FILES: usize = 160;
const MAX_CONTEXT_FILES: usize = 8;
const MAX_CONTEXT_FILE_BYTES: usize = 12_000;

#[derive(Debug, Clone)]
pub struct ContextBundle {
    pub files: Vec<String>,
    pub files_truncated: bool,
    pub file_snippets: Vec<String>,
    pub profiles: Vec<TechProfile>,
    pub integrations: Vec<IntegrationContext>,
}

impl ContextBundle {
    pub fn collect(root: &Path, spec_path: &Path) -> Result<Self> {
        let filesystem = FilesystemContextProvider::new(root, spec_path);
        let mut bundle = filesystem.collect()?;
        bundle.profiles = detect_profiles(&bundle.files);
        bundle.integrations = configured_mcp_integrations(&load_project_config()?.integrations.mcp);
        if bundle.integrations.is_empty() {
            bundle.integrations.push(IntegrationContext {
                id: "mcp".to_string(),
                kind: "external-context".to_string(),
                status: IntegrationStatus::AvailableSlot,
                details: "No external context providers are configured. MCP adapters can add normalized context here for docs, GitHub, database, Figma, or observability providers.".to_string(),
            });
        }
        Ok(bundle)
    }

    pub fn render_project_files_prompt(&self) -> String {
        self.files.join("\n")
    }

    pub fn render_file_snippets_prompt(&self) -> String {
        self.file_snippets.join("\n\n")
    }

    pub fn render_profiles_prompt(&self) -> String {
        render_profiles_prompt(&self.profiles)
    }

    pub fn render_integrations_prompt(&self) -> String {
        if self.integrations.is_empty() {
            return "No integration context providers are configured.".to_string();
        }

        self.integrations
            .iter()
            .map(|integration| {
                format!(
                    "<integration id=\"{}\" kind=\"{}\" status=\"{}\">\n{}\n</integration>",
                    integration.id,
                    integration.kind,
                    integration.status.as_str(),
                    integration.details.trim()
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

fn configured_mcp_integrations(
    configs: &BTreeMap<String, ProjectMcpServerConfig>,
) -> Vec<IntegrationContext> {
    configs
        .iter()
        .map(|(id, config)| {
            let args = if config.args.is_empty() {
                "No args configured.".to_string()
            } else {
                format!("Args: {}.", config.args.join(" "))
            };
            let env_vars = if config.env_vars.is_empty() {
                "No inherited env vars configured.".to_string()
            } else {
                format!("Inherited env vars: {}.", config.env_vars.join(", "))
            };
            let env_keys = if config.env.is_empty() {
                "No inline env overrides configured.".to_string()
            } else {
                let keys = config.env.keys().cloned().collect::<Vec<_>>().join(", ");
                format!("Inline env override keys: {keys}.")
            };
            IntegrationContext {
                id: id.clone(),
                kind: "mcp".to_string(),
                status: if config.enabled {
                    IntegrationStatus::AvailableSlot
                } else {
                    IntegrationStatus::Unavailable
                },
                details: if config.enabled {
                    format!(
                        "Command: {}. {args} {env_vars} {env_keys} Runtime adapter is not active in this build; this MCP server declares an external context source that can be normalized into ContextBundle.",
                        config.command
                    )
                } else {
                    format!(
                        "Command: {}. {args} {env_vars} {env_keys} MCP server is disabled in project config.",
                        config.command
                    )
                },
            }
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationContext {
    pub id: String,
    pub kind: String,
    pub status: IntegrationStatus,
    pub details: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntegrationStatus {
    AvailableSlot,
    Active,
    Unavailable,
}

impl IntegrationStatus {
    fn as_str(&self) -> &'static str {
        match self {
            Self::AvailableSlot => "available-slot",
            Self::Active => "active",
            Self::Unavailable => "unavailable",
        }
    }
}

pub trait ContextProvider {
    fn collect(&self) -> Result<ContextBundle>;
}

#[derive(Debug)]
pub struct FilesystemContextProvider<'a> {
    root: &'a Path,
    spec_path: &'a Path,
}

impl<'a> FilesystemContextProvider<'a> {
    pub fn new(root: &'a Path, spec_path: &'a Path) -> Self {
        Self { root, spec_path }
    }
}

impl ContextProvider for FilesystemContextProvider<'_> {
    fn collect(&self) -> Result<ContextBundle> {
        let mut files = Vec::new();
        collect_project_files(self.root, self.root, self.spec_path, &mut files)?;
        files.sort();
        let files_truncated = files.len() > MAX_PROJECT_FILES;
        files.truncate(MAX_PROJECT_FILES);

        let context_files = select_context_files(&files);
        let mut file_snippets = Vec::new();
        for relative in context_files {
            let path = self.root.join(&relative);
            let Ok(source) = fs::read_to_string(&path) else {
                continue;
            };
            let snippet = truncate_at_char_boundary(&source, MAX_CONTEXT_FILE_BYTES);
            file_snippets.push(format!(
                "<file path=\"{relative}\">\n{}\n</file>",
                snippet.trim()
            ));
        }

        Ok(ContextBundle {
            files,
            files_truncated,
            file_snippets,
            profiles: Vec::new(),
            integrations: Vec::new(),
        })
    }
}

fn collect_project_files(
    root: &Path,
    current: &Path,
    spec_path: &Path,
    files: &mut Vec<String>,
) -> Result<()> {
    let mut entries = fs::read_dir(current)
        .with_context(|| format!("failed to read {}", current.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("failed to read entries under {}", current.display()))?;
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();

        if should_skip_dir_or_file(&name) {
            continue;
        }

        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to inspect {}", path.display()))?;
        if file_type.is_dir() {
            collect_project_files(root, &path, spec_path, files)?;
        } else if file_type.is_file() {
            let relative_path = path.strip_prefix(root).unwrap_or(path.as_path());
            if same_path(root, relative_path, spec_path) {
                continue;
            }
            files.push(relative_path.to_string_lossy().replace('\\', "/"));
        }
    }

    Ok(())
}

fn should_skip_dir_or_file(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".specforge"
            | "target"
            | "node_modules"
            | "dist"
            | "build"
            | ".next"
            | "coverage"
            | "vendor"
            | ".DS_Store"
    )
}

fn same_path(root: &Path, relative_path: &Path, candidate: &Path) -> bool {
    if candidate.is_absolute() {
        return candidate
            .strip_prefix(root)
            .is_ok_and(|candidate_relative| candidate_relative == relative_path);
    }

    normalize_relative_path(candidate) == relative_path
}

fn normalize_relative_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        if let std::path::Component::Normal(value) = component {
            normalized.push(value);
        }
    }

    normalized
}

fn select_context_files(files: &[String]) -> Vec<String> {
    let mut selected = Vec::new();
    let priority_names = [
        "README.md",
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "go.mod",
        "pom.xml",
        "Makefile",
        "src/main.rs",
        "src/lib.rs",
        "src/main.ts",
        "src/main.tsx",
        "src/App.tsx",
        "src/app.js",
        "index.html",
    ];

    for priority in priority_names {
        if selected.len() >= MAX_CONTEXT_FILES {
            break;
        }
        if files.iter().any(|file| file == priority) {
            selected.push(priority.to_string());
        }
    }

    for file in files {
        if selected.len() >= MAX_CONTEXT_FILES {
            break;
        }
        if selected.iter().any(|selected_file| selected_file == file) {
            continue;
        }
        if is_likely_source_or_doc(file) {
            selected.push(file.clone());
        }
    }

    selected
}

fn is_likely_source_or_doc(file: &str) -> bool {
    matches!(
        Path::new(file)
            .extension()
            .and_then(|extension| extension.to_str()),
        Some(
            "adoc"
                | "md"
                | "rs"
                | "js"
                | "jsx"
                | "ts"
                | "tsx"
                | "py"
                | "go"
                | "java"
                | "kt"
                | "rb"
                | "php"
                | "swift"
                | "cs"
                | "toml"
                | "yaml"
                | "yml"
                | "json"
                | "html"
                | "css"
        )
    )
}

fn truncate_at_char_boundary(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }

    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prioritizes_project_context_files() {
        let files = vec![
            "src/feature.rs".to_string(),
            "README.md".to_string(),
            "Cargo.toml".to_string(),
            "src/main.rs".to_string(),
        ];

        assert_eq!(
            select_context_files(&files),
            vec!["README.md", "Cargo.toml", "src/main.rs", "src/feature.rs"]
        );
    }

    #[test]
    fn truncates_at_utf8_boundary() {
        assert_eq!(truncate_at_char_boundary("aéz", 2), "a");
    }

    #[test]
    fn detects_selected_spec_path_variants() {
        let root = Path::new("/repo");
        let relative = Path::new("spec.adoc");

        assert!(same_path(root, relative, Path::new("spec.adoc")));
        assert!(same_path(root, relative, Path::new("./spec.adoc")));
        assert!(same_path(root, relative, Path::new("/repo/spec.adoc")));
    }

    #[test]
    fn renders_configured_mcp_integrations_without_env_values() {
        let integrations = configured_mcp_integrations(&BTreeMap::from([(
            "context7".to_string(),
            ProjectMcpServerConfig {
                command: "npx".to_string(),
                enabled: true,
                args: vec!["-y".to_string(), "@upstash/context7-mcp".to_string()],
                env_vars: vec!["LOCAL_TOKEN".to_string()],
                env: BTreeMap::from([("MY_ENV_VAR".to_string(), "MY_ENV_VALUE".to_string())]),
            },
        )]));

        assert_eq!(integrations.len(), 1);
        assert_eq!(integrations[0].id, "context7");
        assert_eq!(integrations[0].kind, "mcp");
        assert_eq!(integrations[0].status, IntegrationStatus::AvailableSlot);
        assert!(integrations[0].details.contains("@upstash/context7-mcp"));
        assert!(integrations[0].details.contains("LOCAL_TOKEN"));
        assert!(integrations[0].details.contains("MY_ENV_VAR"));
        assert!(!integrations[0].details.contains("MY_ENV_VALUE"));
    }
}
