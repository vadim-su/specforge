use std::{fs, path::Path};

use anyhow::{Context, Result};

use crate::{
    config::{CURRENT_MODEL, CURRENT_SPEC, STATE_DIR},
    spec::ParsedSpec,
};

pub fn write_current_state(parsed: &ParsedSpec) -> Result<()> {
    fs::create_dir_all(STATE_DIR).context("failed to create .specforge state directory")?;
    fs::write(CURRENT_SPEC, &parsed.source).context("failed to write current spec snapshot")?;
    fs::write(CURRENT_MODEL, serde_json::to_string_pretty(&parsed.model)?)
        .context("failed to write current model snapshot")?;

    Ok(())
}

pub fn clear_current_state() -> Result<()> {
    remove_if_exists(CURRENT_SPEC)?;
    remove_if_exists(CURRENT_MODEL)?;

    Ok(())
}

fn remove_if_exists(path: &str) -> Result<()> {
    if Path::new(path).exists() {
        fs::remove_file(path).with_context(|| format!("failed to remove {path}"))?;
    }

    Ok(())
}
