use std::{fs, path::Path};

use anyhow::{Context, Result};
use rig::message::ToolCall;
use serde_json::{Value, json};

const MAX_FILE_LINES: usize = 160;

pub(super) fn list_project_files(call: &ToolCall, protected_paths: &[String]) -> Result<Value> {
    let limit = call
        .function
        .arguments
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(120)
        .min(500) as usize;
    let root = std::env::current_dir().context("failed to read current directory")?;
    let mut files = Vec::new();
    collect_project_files(&root, &root, limit, protected_paths, &mut files)?;

    Ok(json!({
        "ok": true,
        "files": files,
        "truncated": files.len() >= limit,
    }))
}

pub(super) fn inspect_file(call: &ToolCall) -> Result<Value> {
    let Some(path) = call.function.arguments.get("path").and_then(Value::as_str) else {
        return Ok(json!({
            "ok": false,
            "error": "missing string argument `path`",
        }));
    };

    if !super::is_safe_relative_path(Path::new(path)) {
        return Ok(json!({
            "ok": false,
            "error": "path must be relative and stay inside the repository",
        }));
    }

    let start_line = call
        .function
        .arguments
        .get("start_line")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1) as usize;
    let requested_end = call
        .function
        .arguments
        .get("end_line")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(start_line + MAX_FILE_LINES - 1);
    let end_line = requested_end.min(start_line + MAX_FILE_LINES - 1);
    let full_path = std::env::current_dir()
        .context("failed to read current directory")?
        .join(path);
    let source = fs::read_to_string(&full_path)
        .with_context(|| format!("failed to read {}", full_path.display()))?;
    let lines = source
        .lines()
        .enumerate()
        .filter_map(|(idx, line)| {
            let line_number = idx + 1;
            (line_number >= start_line && line_number <= end_line)
                .then(|| json!({ "line": line_number, "text": line }))
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "ok": true,
        "path": path,
        "start_line": start_line,
        "end_line": end_line,
        "lines": lines,
    }))
}

fn collect_project_files(
    root: &Path,
    current: &Path,
    limit: usize,
    protected_paths: &[String],
    files: &mut Vec<String>,
) -> Result<()> {
    if files.len() >= limit {
        return Ok(());
    }

    let mut entries = fs::read_dir(current)
        .with_context(|| format!("failed to read {}", current.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("failed to read entries under {}", current.display()))?;
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        if files.len() >= limit {
            break;
        }

        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();

        if matches!(name.as_ref(), ".git" | ".specforge" | "target") {
            continue;
        }

        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to inspect {}", path.display()))?;
        if file_type.is_dir() {
            collect_project_files(root, &path, limit, protected_paths, files)?;
        } else if file_type.is_file() {
            let relative = path
                .strip_prefix(root)
                .unwrap_or(path.as_path())
                .to_string_lossy()
                .replace('\\', "/");
            if super::is_specforge_owned_path(Path::new(&relative), protected_paths) {
                continue;
            }
            files.push(relative);
        }
    }

    Ok(())
}
