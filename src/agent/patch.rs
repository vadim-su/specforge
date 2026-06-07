use std::{fs, path::Path};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(super) enum PatchOperation {
    Add,
    Update,
    Delete,
    Move,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct PatchFileChange {
    pub operation: PatchOperation,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ProposedPatch {
    pub summary: String,
    pub patch: String,
    pub changes: Vec<PatchFileChange>,
    pub check: Option<super::checks::ProjectCheckRun>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CurrentPatchHeader {
    Add,
    Update,
    Delete,
}

#[derive(Debug)]
enum ParsedPatchOp {
    Add {
        path: String,
        lines: Vec<String>,
    },
    Delete {
        path: String,
    },
    Update {
        path: String,
        move_to: Option<String>,
        hunks: Vec<PatchHunk>,
    },
}

#[derive(Debug)]
struct PatchHunk {
    lines: Vec<PatchHunkLine>,
}

#[derive(Debug)]
enum PatchHunkLine {
    Context(String),
    Removed(String),
    Added(String),
}

#[cfg(test)]
pub(super) fn validate_apply_patch(
    patch: &str,
) -> std::result::Result<Vec<PatchFileChange>, String> {
    validate_apply_patch_with_protected_paths(patch, &[])
}

pub(super) fn validate_apply_patch_with_protected_paths(
    patch: &str,
    protected_paths: &[String],
) -> std::result::Result<Vec<PatchFileChange>, String> {
    let lines = patch.trim().lines().collect::<Vec<_>>();
    if lines.first().map(|line| line.trim()) != Some("*** Begin Patch") {
        return Err("patch must start with `*** Begin Patch`".to_string());
    }
    if lines.last().map(|line| line.trim()) != Some("*** End Patch") {
        return Err("patch must end with `*** End Patch`".to_string());
    }

    let mut changes = Vec::new();
    let mut current_header = None;
    let mut saw_operation = false;

    for line in &lines[1..lines.len().saturating_sub(1)] {
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            let path = validate_patch_path_with_protected_paths(path, protected_paths)?;
            current_header = Some(CurrentPatchHeader::Add);
            saw_operation = true;
            changes.push(PatchFileChange {
                operation: PatchOperation::Add,
                path,
            });
        } else if let Some(path) = line.strip_prefix("*** Update File: ") {
            let path = validate_patch_path_with_protected_paths(path, protected_paths)?;
            current_header = Some(CurrentPatchHeader::Update);
            saw_operation = true;
            changes.push(PatchFileChange {
                operation: PatchOperation::Update,
                path,
            });
        } else if let Some(path) = line.strip_prefix("*** Delete File: ") {
            let path = validate_patch_path_with_protected_paths(path, protected_paths)?;
            current_header = Some(CurrentPatchHeader::Delete);
            saw_operation = true;
            changes.push(PatchFileChange {
                operation: PatchOperation::Delete,
                path,
            });
        } else if let Some(path) = line.strip_prefix("*** Move to: ") {
            if current_header != Some(CurrentPatchHeader::Update) {
                return Err("`*** Move to:` must follow `*** Update File:`".to_string());
            }
            let path = validate_patch_path_with_protected_paths(path, protected_paths)?;
            changes.push(PatchFileChange {
                operation: PatchOperation::Move,
                path,
            });
        } else if *line == "*** End of File" {
            continue;
        } else if line.starts_with("*** ") {
            return Err(format!("unsupported patch header `{line}`"));
        }
    }

    if !saw_operation {
        return Err("patch must include at least one file operation".to_string());
    }

    Ok(changes)
}

fn validate_patch_path_with_protected_paths(
    path: &str,
    protected_paths: &[String],
) -> std::result::Result<String, String> {
    let path = path.trim();
    if path.is_empty() {
        return Err("patch file path must not be empty".to_string());
    }

    let path = Path::new(path);
    if !super::path_policy::is_safe_relative_path(path) {
        return Err(format!(
            "patch path `{}` must be relative and stay inside the project",
            path.display()
        ));
    }
    if path
        .components()
        .next()
        .is_some_and(|component| component.as_os_str() == ".git")
    {
        return Err("patch must not write inside .git".to_string());
    }
    if super::path_policy::is_specforge_owned_path(path, protected_paths) {
        return Err(format!(
            "patch must not modify SpecForge-owned path `{}`",
            path.display()
        ));
    }

    Ok(path.to_string_lossy().replace('\\', "/"))
}

pub(super) fn apply_proposed_patch(
    proposed_patch: &ProposedPatch,
    protected_paths: &[String],
) -> Result<()> {
    let operations = parse_patch_operations(&proposed_patch.patch, protected_paths)?;
    for operation in operations {
        match operation {
            ParsedPatchOp::Add { path, lines } => {
                let path = Path::new(&path);
                if path.exists() {
                    bail!("cannot add {}; file already exists", path.display());
                }
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)
                        .with_context(|| format!("failed to create {}", parent.display()))?;
                }
                fs::write(path, super::ensure_trailing_newline(&lines.join("\n")))
                    .with_context(|| format!("failed to write {}", path.display()))?;
            }
            ParsedPatchOp::Delete { path } => {
                let path = Path::new(&path);
                fs::remove_file(path)
                    .with_context(|| format!("failed to delete {}", path.display()))?;
            }
            ParsedPatchOp::Update {
                path,
                move_to,
                hunks,
            } => {
                apply_update_operation(&path, move_to.as_deref(), &hunks)?;
            }
        }
    }

    Ok(())
}

fn parse_patch_operations(patch: &str, protected_paths: &[String]) -> Result<Vec<ParsedPatchOp>> {
    validate_apply_patch_with_protected_paths(patch, protected_paths)
        .map_err(anyhow::Error::msg)?;
    let lines = patch.trim().lines().collect::<Vec<_>>();
    let mut operations = Vec::new();
    let mut index = 1;

    while index + 1 < lines.len() {
        let line = lines[index];
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            let path = validate_patch_path_with_protected_paths(path, protected_paths)
                .map_err(anyhow::Error::msg)?;
            index += 1;
            let mut added = Vec::new();
            while index + 1 < lines.len() && !is_file_operation_header(lines[index]) {
                let Some(line) = lines[index].strip_prefix('+') else {
                    bail!("add operation for {path} contains a non-added line");
                };
                added.push(line.to_string());
                index += 1;
            }
            operations.push(ParsedPatchOp::Add { path, lines: added });
        } else if let Some(path) = line.strip_prefix("*** Delete File: ") {
            let path = validate_patch_path_with_protected_paths(path, protected_paths)
                .map_err(anyhow::Error::msg)?;
            operations.push(ParsedPatchOp::Delete { path });
            index += 1;
        } else if let Some(path) = line.strip_prefix("*** Update File: ") {
            let path = validate_patch_path_with_protected_paths(path, protected_paths)
                .map_err(anyhow::Error::msg)?;
            index += 1;
            let mut move_to = None;
            if index + 1 < lines.len()
                && let Some(move_path) = lines[index].strip_prefix("*** Move to: ")
            {
                move_to = Some(
                    validate_patch_path_with_protected_paths(move_path, protected_paths)
                        .map_err(anyhow::Error::msg)?,
                );
                index += 1;
            }
            let mut hunks = Vec::new();
            while index + 1 < lines.len() && !is_file_operation_header(lines[index]) {
                if !lines[index].starts_with("@@") {
                    bail!("update operation for {path} expected hunk header");
                }
                index += 1;
                let mut hunk_lines = Vec::new();
                while index + 1 < lines.len()
                    && !lines[index].starts_with("@@")
                    && !is_file_operation_header(lines[index])
                {
                    let line = lines[index];
                    if line == "*** End of File" {
                        index += 1;
                        continue;
                    }
                    let Some(prefix) = line.chars().next() else {
                        hunk_lines.push(PatchHunkLine::Context(String::new()));
                        index += 1;
                        continue;
                    };
                    let content = line.get(1..).unwrap_or_default().to_string();
                    match prefix {
                        ' ' => hunk_lines.push(PatchHunkLine::Context(content)),
                        '-' => hunk_lines.push(PatchHunkLine::Removed(content)),
                        '+' => hunk_lines.push(PatchHunkLine::Added(content)),
                        _ => bail!("invalid hunk line prefix `{prefix}` in {path}"),
                    }
                    index += 1;
                }
                hunks.push(PatchHunk { lines: hunk_lines });
            }
            operations.push(ParsedPatchOp::Update {
                path,
                move_to,
                hunks,
            });
        } else {
            bail!("unsupported patch line `{line}`");
        }
    }

    Ok(operations)
}

fn is_file_operation_header(line: &str) -> bool {
    line.starts_with("*** Add File: ")
        || line.starts_with("*** Update File: ")
        || line.starts_with("*** Delete File: ")
        || line == "*** End Patch"
}

fn apply_update_operation(path: &str, move_to: Option<&str>, hunks: &[PatchHunk]) -> Result<()> {
    let path_ref = Path::new(path);
    let source = fs::read_to_string(path_ref)
        .with_context(|| format!("failed to read {}", path_ref.display()))?;
    let had_trailing_newline = source.ends_with('\n');
    let mut lines = source.lines().map(str::to_string).collect::<Vec<_>>();
    let mut cursor = 0;

    for hunk in hunks {
        let old_lines = hunk
            .lines
            .iter()
            .filter_map(|line| match line {
                PatchHunkLine::Context(text) | PatchHunkLine::Removed(text) => Some(text.clone()),
                PatchHunkLine::Added(_) => None,
            })
            .collect::<Vec<_>>();
        let new_lines = hunk
            .lines
            .iter()
            .filter_map(|line| match line {
                PatchHunkLine::Context(text) | PatchHunkLine::Added(text) => Some(text.clone()),
                PatchHunkLine::Removed(_) => None,
            })
            .collect::<Vec<_>>();
        let Some(start) = find_subsequence(&lines, &old_lines, cursor) else {
            bail!("failed to locate update hunk in {path}");
        };
        let end = start + old_lines.len();
        lines.splice(start..end, new_lines.clone());
        cursor = start + new_lines.len();
    }

    let mut output = lines.join("\n");
    if had_trailing_newline {
        output.push('\n');
    }

    let destination = move_to.map(Path::new).unwrap_or(path_ref);
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(destination, output)
        .with_context(|| format!("failed to write {}", destination.display()))?;
    if let Some(destination) = move_to
        && destination != path
    {
        fs::remove_file(path_ref)
            .with_context(|| format!("failed to remove {}", path_ref.display()))?;
    }

    Ok(())
}

fn find_subsequence(lines: &[String], needle: &[String], start: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(start.min(lines.len()));
    }
    lines
        .windows(needle.len())
        .enumerate()
        .skip(start)
        .find_map(|(index, window)| (window == needle).then_some(index))
}
