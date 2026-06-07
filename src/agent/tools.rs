use rig::{tool::ToolDyn, tool_macro};
use serde_json::Value;

#[derive(Debug)]
struct SpecForgeToolError;

impl std::fmt::Display for SpecForgeToolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SpecForge executes development tools in its manual agent loop")
    }
}

impl std::error::Error for SpecForgeToolError {}

/// Return the semantic diff between the previous current spec and the newly stored current spec.
#[tool_macro]
fn inspect_spec_diff() -> Result<Value, SpecForgeToolError> {
    Err(SpecForgeToolError)
}

/// Return one target spec item by stable ID, including its source excerpt.
#[tool_macro(required(id))]
fn inspect_spec_item(
    /// Stable spec item ID, for example feat.todo-management
    id: String,
) -> Result<Value, SpecForgeToolError> {
    let _ = id;
    Err(SpecForgeToolError)
}

/// List repository files allowed by .specforge/config.yaml file_access.allowed, excluding .git, target, .specforge, and SpecForge-owned spec files.
#[tool_macro]
fn list_project_files(
    /// Maximum number of files to return.
    limit: Option<u64>,
) -> Result<Value, SpecForgeToolError> {
    let _ = limit;
    Err(SpecForgeToolError)
}

/// Return a bounded excerpt from an allowed repository file by relative path and line range.
#[tool_macro(required(path))]
fn inspect_file(
    /// Relative repository path.
    path: String,
    /// First line to include, 1-based.
    start_line: Option<u64>,
    /// Last line to include, 1-based.
    end_line: Option<u64>,
) -> Result<Value, SpecForgeToolError> {
    let _ = (path, start_line, end_line);
    Err(SpecForgeToolError)
}

/// Validate and apply one Codex apply_patch patch, then run project checks and return the result.
#[tool_macro(required(summary, patch))]
fn propose_patch(
    /// Short human-readable patch summary.
    summary: String,
    /// Complete Codex apply_patch patch text.
    patch: String,
) -> Result<Value, SpecForgeToolError> {
    let _ = (summary, patch);
    Err(SpecForgeToolError)
}

pub(super) fn development_tools() -> Vec<Box<dyn ToolDyn>> {
    vec![
        Box::new(InspectSpecDiff),
        Box::new(InspectSpecItem),
        Box::new(ListProjectFiles),
        Box::new(InspectFile),
        Box::new(ProposePatch),
    ]
}

pub(super) fn code_change_tools() -> Vec<Box<dyn ToolDyn>> {
    vec![
        Box::new(ListProjectFiles),
        Box::new(InspectFile),
        Box::new(ProposePatch),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn declarative_tools_keep_expected_names() {
        let tools = development_tools();
        let mut names = Vec::new();
        for tool in tools {
            names.push(tool.definition(String::new()).await.name);
        }

        assert_eq!(
            names,
            vec![
                "inspect_spec_diff",
                "inspect_spec_item",
                "list_project_files",
                "inspect_file",
                "propose_patch"
            ]
        );
    }
}
