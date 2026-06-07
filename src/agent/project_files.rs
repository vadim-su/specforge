use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result};
use rig::message::ToolCall;
use serde_json::{Value, json};

const MAX_FILE_LINES: usize = 160;

pub(super) fn list_project_files(
    call: &ToolCall,
    protected_paths: &[String],
    allowed_paths: &[String],
) -> Result<Value> {
    let limit = call
        .function
        .arguments
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(120)
        .min(500) as usize;
    let root = std::env::current_dir().context("failed to read current directory")?;
    let mut files = Vec::new();
    collect_project_files(
        &root,
        &root,
        limit,
        protected_paths,
        allowed_paths,
        &mut files,
    )?;

    Ok(json!({
        "ok": true,
        "files": files,
        "truncated": files.len() >= limit,
    }))
}

pub(super) fn inspect_file(
    call: &ToolCall,
    protected_paths: &[String],
    allowed_paths: &[String],
) -> Result<Value> {
    let Some(path) = call.function.arguments.get("path").and_then(Value::as_str) else {
        return Ok(json!({
            "ok": false,
            "error": "missing string argument `path`",
        }));
    };

    let requested_path = Path::new(path);
    if !super::path_policy::is_safe_relative_path(requested_path) {
        return Ok(json!({
            "ok": false,
            "error": "path must be relative and stay inside the repository",
        }));
    }
    let path = normalize_requested_path(requested_path);
    if path.is_empty() {
        return Ok(json!({
            "ok": false,
            "error": "path must name a repository file",
        }));
    }
    if super::path_policy::is_specforge_owned_path(Path::new(&path), protected_paths) {
        return Ok(json!({
            "ok": false,
            "error": "path is reserved for SpecForge state/spec management",
        }));
    }
    if !file_access_allows_path(&path, allowed_paths) {
        return Ok(json!({
            "ok": false,
            "error": "path is not allowed by .specforge/config.yaml file_access.allowed",
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
        .join(&path);
    let bytes =
        fs::read(&full_path).with_context(|| format!("failed to read {}", full_path.display()))?;
    let Ok(source) = String::from_utf8(bytes) else {
        return Ok(json!({
            "ok": false,
            "error": "file is not valid UTF-8 text",
            "path": path,
        }));
    };
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

fn normalize_requested_path(path: &Path) -> String {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            _ => {}
        }
    }

    normalized.to_string_lossy().replace('\\', "/")
}

fn collect_project_files(
    root: &Path,
    current: &Path,
    limit: usize,
    protected_paths: &[String],
    allowed_paths: &[String],
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
            collect_project_files(root, &path, limit, protected_paths, allowed_paths, files)?;
        } else if file_type.is_file() {
            let relative = path
                .strip_prefix(root)
                .unwrap_or(path.as_path())
                .to_string_lossy()
                .replace('\\', "/");
            if super::path_policy::is_specforge_owned_path(Path::new(&relative), protected_paths) {
                continue;
            }
            if !file_access_allows_path(&relative, allowed_paths) {
                continue;
            }
            if !is_probably_text_file(&path)? {
                continue;
            }
            files.push(relative);
        }
    }

    Ok(())
}

fn is_probably_text_file(path: &Path) -> Result<bool> {
    const SAMPLE_BYTES: usize = 8192;

    let mut file =
        fs::File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut buffer = [0; SAMPLE_BYTES];
    let bytes_read = file
        .read(&mut buffer)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let sample = &buffer[..bytes_read];

    Ok(!sample.contains(&0) && std::str::from_utf8(sample).is_ok())
}

fn file_access_allows_path(path: &str, allowed_paths: &[String]) -> bool {
    if allowed_paths.is_empty() {
        return true;
    }

    let normalized = path.replace('\\', "/");
    allowed_paths.iter().any(|allowed_path| {
        if let Some(prefix) = allowed_path.strip_suffix('/') {
            normalized == prefix || normalized.starts_with(allowed_path)
        } else {
            normalized == *allowed_path
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{env, process};

    #[test]
    fn empty_file_access_allows_any_path() {
        assert!(file_access_allows_path("src/lib.rs", &[]));
    }

    #[test]
    fn file_access_allows_exact_files() {
        assert!(file_access_allows_path(
            "Cargo.toml",
            &["Cargo.toml".to_string()]
        ));
        assert!(!file_access_allows_path(
            "Cargo.lock",
            &["Cargo.toml".to_string()]
        ));
    }

    #[test]
    fn file_access_allows_directory_descendants() {
        assert!(file_access_allows_path("src/lib.rs", &["src/".to_string()]));
        assert!(file_access_allows_path("src", &["src/".to_string()]));
        assert!(!file_access_allows_path(
            "scripts/build.rs",
            &["src/".to_string()]
        ));
    }

    #[test]
    fn normalizes_requested_file_paths() {
        assert_eq!(
            normalize_requested_path(Path::new("./src/lib.rs")),
            "src/lib.rs"
        );
    }

    #[test]
    fn detects_probable_text_files() {
        let path = env::temp_dir().join(format!(
            "specforge-text-file-{}-{}.txt",
            process::id(),
            "text"
        ));
        fs::write(&path, "plain UTF-8 text\n").unwrap();

        assert!(is_probably_text_file(&path).unwrap());

        let _ = fs::remove_file(path);
    }

    #[test]
    fn rejects_binary_files_with_nul_bytes() {
        let path = env::temp_dir().join(format!(
            "specforge-binary-file-{}-{}.bin",
            process::id(),
            "binary"
        ));
        fs::write(&path, [0x7f, b'E', b'L', b'F', 0, 1, 2, 3]).unwrap();

        assert!(!is_probably_text_file(&path).unwrap());

        let _ = fs::remove_file(path);
    }
}
