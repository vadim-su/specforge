use rig::{
    completion::ToolDefinition,
    tool::{Tool, ToolDyn},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug)]
struct SpecForgeToolError;

impl std::fmt::Display for SpecForgeToolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SpecForge executes development tools in its manual agent loop")
    }
}

impl std::error::Error for SpecForgeToolError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InspectSpecDiffTool;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InspectSpecDiffArgs {}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InspectSpecItemTool;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InspectSpecItemArgs {
    id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ListProjectFilesTool;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ListProjectFilesArgs {
    limit: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InspectFileTool;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InspectFileArgs {
    path: String,
    start_line: Option<u64>,
    end_line: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProposePatchTool;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProposePatchArgs {
    summary: String,
    patch: String,
}

impl Tool for InspectSpecDiffTool {
    const NAME: &'static str = "inspect_spec_diff";
    type Error = SpecForgeToolError;
    type Args = InspectSpecDiffArgs;
    type Output = Value;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Return the semantic diff between the previous current spec and the newly stored current spec.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> std::result::Result<Self::Output, Self::Error> {
        Err(SpecForgeToolError)
    }
}

impl Tool for InspectSpecItemTool {
    const NAME: &'static str = "inspect_spec_item";
    type Error = SpecForgeToolError;
    type Args = InspectSpecItemArgs;
    type Output = Value;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Return one target spec item by stable ID, including its source excerpt."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Stable spec item ID, for example feat.todo-management" }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> std::result::Result<Self::Output, Self::Error> {
        Err(SpecForgeToolError)
    }
}

impl Tool for ListProjectFilesTool {
    const NAME: &'static str = "list_project_files";
    type Error = SpecForgeToolError;
    type Args = ListProjectFilesArgs;
    type Output = Value;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "List repository files, excluding .git, target, .specforge, and SpecForge-owned spec files.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "minimum": 1, "maximum": 500 }
                },
                "additionalProperties": false
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> std::result::Result<Self::Output, Self::Error> {
        Err(SpecForgeToolError)
    }
}

impl Tool for InspectFileTool {
    const NAME: &'static str = "inspect_file";
    type Error = SpecForgeToolError;
    type Args = InspectFileArgs;
    type Output = Value;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description:
                "Return a bounded repository file excerpt by relative path and line range."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative repository path" },
                    "start_line": { "type": "integer", "minimum": 1 },
                    "end_line": { "type": "integer", "minimum": 1 }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> std::result::Result<Self::Output, Self::Error> {
        Err(SpecForgeToolError)
    }
}

impl Tool for ProposePatchTool {
    const NAME: &'static str = "propose_patch";
    type Error = SpecForgeToolError;
    type Args = ProposePatchArgs;
    type Output = Value;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Validate and apply one Codex apply_patch patch, then run project checks and return the result.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "summary": { "type": "string", "description": "Short human-readable patch summary" },
                    "patch": { "type": "string", "description": "Complete Codex apply_patch patch text" }
                },
                "required": ["summary", "patch"],
                "additionalProperties": false
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> std::result::Result<Self::Output, Self::Error> {
        Err(SpecForgeToolError)
    }
}

pub(super) fn development_tools() -> Vec<Box<dyn ToolDyn>> {
    vec![
        Box::new(InspectSpecDiffTool),
        Box::new(InspectSpecItemTool),
        Box::new(ListProjectFilesTool),
        Box::new(InspectFileTool),
        Box::new(ProposePatchTool),
    ]
}

pub(super) fn code_change_tools() -> Vec<Box<dyn ToolDyn>> {
    vec![
        Box::new(ListProjectFilesTool),
        Box::new(InspectFileTool),
        Box::new(ProposePatchTool),
    ]
}
