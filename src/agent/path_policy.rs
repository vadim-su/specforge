use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

pub(super) fn is_safe_relative_path(path: &Path) -> bool {
    !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

pub(super) fn normalize_protected_paths(paths: &[PathBuf]) -> Result<Vec<String>> {
    paths
        .iter()
        .filter_map(|path| normalize_protected_path(path).transpose())
        .collect()
}

fn normalize_protected_path(path: &Path) -> Result<Option<String>> {
    let current_dir = std::env::current_dir().context("failed to read current directory")?;
    let relative = if path.is_absolute() {
        let Ok(stripped) = path.strip_prefix(&current_dir) else {
            return Ok(None);
        };
        stripped
    } else {
        path
    };

    if !is_safe_relative_path(relative) {
        bail!(
            "protected path must be relative to the project root: {}",
            path.display()
        );
    }

    let mut normalized = PathBuf::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            _ => bail!(
                "protected path must stay inside the project root: {}",
                path.display()
            ),
        }
    }

    if normalized.as_os_str().is_empty() {
        return Ok(None);
    }

    Ok(Some(normalized.to_string_lossy().replace('\\', "/")))
}

pub(super) fn normalize_allowed_paths(paths: &[String]) -> Result<Vec<String>> {
    paths
        .iter()
        .filter_map(|path| normalize_allowed_path(path).transpose())
        .collect()
}

fn normalize_allowed_path(path: &str) -> Result<Option<String>> {
    let path = path.trim();
    if path.is_empty() {
        return Ok(None);
    }

    let directory_rule = path.ends_with('/') || path.ends_with("/**");
    let path = path
        .strip_suffix("/**")
        .unwrap_or(path)
        .trim_end_matches('/');
    let path = Path::new(path);

    if !is_safe_relative_path(path) {
        bail!(
            "file_access.allowed path must be relative to the project root: {}",
            path.display()
        );
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            _ => bail!(
                "file_access.allowed path must stay inside the project root: {}",
                path.display()
            ),
        }
    }

    if normalized.as_os_str().is_empty() {
        return Ok(None);
    }

    let mut normalized = normalized.to_string_lossy().replace('\\', "/");
    if directory_rule || Path::new(&normalized).is_dir() {
        normalized.push('/');
    }

    Ok(Some(normalized))
}

pub(super) fn is_specforge_owned_path(path: &Path, protected_paths: &[String]) -> bool {
    let normalized = path.to_string_lossy().replace('\\', "/");
    if protected_paths
        .iter()
        .any(|protected_path| protected_path == &normalized)
    {
        return true;
    }

    if path
        .components()
        .next()
        .is_some_and(|component| component.as_os_str() == ".specforge")
    {
        return true;
    }

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();

    matches!(file_name, "spec.adoc" | "spec.asciidoc")
        || file_name.ends_with(".spec.adoc")
        || file_name.ends_with(".spec.asciidoc")
}
