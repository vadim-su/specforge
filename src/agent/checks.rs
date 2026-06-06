use std::{
    path::Path,
    process::{Command as ProcessCommand, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const CHECK_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_CHECK_OUTPUT_CHARS: usize = 8_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct CheckRun {
    pub command: Vec<String>,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub skipped_reason: Option<String>,
    pub stdout_tail: String,
    pub stderr_tail: String,
}

pub(super) fn run_project_checks() -> Result<CheckRun> {
    if Path::new("Cargo.toml").exists() {
        return run_check_command(&["cargo", "test", "--color", "never"]);
    }

    if Path::new("package.json").exists() {
        return run_check_command(&["npm", "test"]);
    }

    if Path::new("go.mod").exists() {
        return run_check_command(&["go", "test", "./..."]);
    }

    Ok(CheckRun {
        command: Vec::new(),
        success: true,
        exit_code: None,
        timed_out: false,
        skipped_reason: Some(
            "no known project check command found (Cargo.toml, package.json, or go.mod)"
                .to_string(),
        ),
        stdout_tail: String::new(),
        stderr_tail: String::new(),
    })
}

fn run_check_command(command: &[&str]) -> Result<CheckRun> {
    let Some((program, args)) = command.split_first() else {
        bail!("check command must not be empty");
    };

    let mut child = ProcessCommand::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start check command `{}`", command.join(" ")))?;
    let start = Instant::now();
    let mut timed_out = false;

    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if start.elapsed() >= CHECK_TIMEOUT {
            timed_out = true;
            child
                .kill()
                .with_context(|| format!("failed to stop check command `{}`", command.join(" ")))?;
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }

    let output = child
        .wait_with_output()
        .with_context(|| format!("failed to collect check output `{}`", command.join(" ")))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    Ok(CheckRun {
        command: command.iter().map(|part| (*part).to_string()).collect(),
        success: output.status.success() && !timed_out,
        exit_code: output.status.code(),
        timed_out,
        skipped_reason: None,
        stdout_tail: tail_chars(&stdout, MAX_CHECK_OUTPUT_CHARS),
        stderr_tail: tail_chars(&stderr, MAX_CHECK_OUTPUT_CHARS),
    })
}

fn tail_chars(text: &str, max_chars: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= max_chars {
        return text.to_string();
    }

    text.chars().skip(char_count - max_chars).collect()
}
