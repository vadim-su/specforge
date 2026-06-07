use std::{
    process::{Command as ProcessCommand, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::config::{ProjectCheckConfig, load_project_config};

const MAX_CHECK_OUTPUT_CHARS: usize = 8_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ProjectCheckRun {
    pub success: bool,
    pub skipped_reason: Option<String>,
    pub checks: Vec<CheckRun>,
}

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

#[derive(Debug, Clone, Serialize)]
pub(super) struct ProjectCheckPlan {
    pub source: String,
    pub skipped_reason: Option<String>,
    pub checks: Vec<ProjectCheckCommand>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct ProjectCheckCommand {
    pub command: Vec<String>,
    pub timeout_seconds: u64,
}

pub(super) fn project_check_plan() -> Result<ProjectCheckPlan> {
    let config = load_project_config()?;
    if !config.checks.is_empty() {
        return Ok(ProjectCheckPlan {
            source: crate::config::CONFIG_FILE.to_string(),
            skipped_reason: None,
            checks: normalize_check_configs(&config.checks)?,
        });
    }

    Ok(ProjectCheckPlan {
        source: crate::config::CONFIG_FILE.to_string(),
        skipped_reason: Some(format!(
            "{} does not define checks",
            crate::config::CONFIG_FILE
        )),
        checks: Vec::new(),
    })
}

pub(super) fn run_project_checks() -> Result<ProjectCheckRun> {
    let plan = project_check_plan()?;
    if let Some(reason) = plan.skipped_reason {
        return Ok(ProjectCheckRun {
            success: true,
            skipped_reason: Some(reason),
            checks: Vec::new(),
        });
    }

    let mut checks = Vec::new();
    for check in plan.checks {
        checks.push(run_check_command(
            &check.command,
            Duration::from_secs(check.timeout_seconds),
        )?);
    }
    let success = checks.iter().all(|check| check.success);

    Ok(ProjectCheckRun {
        success,
        skipped_reason: None,
        checks,
    })
}

fn normalize_check_configs(configs: &[ProjectCheckConfig]) -> Result<Vec<ProjectCheckCommand>> {
    let mut checks = Vec::new();
    for (index, config) in configs.iter().enumerate() {
        if config.command.is_empty() {
            bail!(
                "{} checks[{}].command must not be empty",
                crate::config::CONFIG_FILE,
                index
            );
        }
        if config.timeout_seconds == 0 {
            bail!(
                "{} checks[{}].timeout_seconds must be greater than 0",
                crate::config::CONFIG_FILE,
                index
            );
        }

        checks.push(ProjectCheckCommand {
            command: config.command.clone(),
            timeout_seconds: config.timeout_seconds,
        });
    }

    Ok(checks)
}

fn run_check_command(command: &[String], timeout: Duration) -> Result<CheckRun> {
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
        if start.elapsed() >= timeout {
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
        command: command.to_vec(),
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
