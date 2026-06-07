use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use rig::message::Message;
use serde::{Deserialize, Serialize};

use super::{TaskChecklistItem, TaskStepStatus, ensure_trailing_newline, patch::ProposedPatch};
use crate::config::TASKS_DIR;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(super) enum TaskStatus {
    Running,
    Completed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(super) enum TaskKind {
    SpecSync,
    CodeChange,
    TestCoverage,
}

fn default_task_kind() -> TaskKind {
    TaskKind::SpecSync
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct AgentTaskState {
    #[serde(default = "default_task_kind")]
    pub(super) kind: TaskKind,
    pub(super) task_id: String,
    pub(super) previous_current_spec_hash: String,
    pub(super) target_spec_hash: String,
    pub(super) max_steps: usize,
    #[serde(default)]
    pub(super) protected_paths: Vec<String>,
    #[serde(default)]
    pub(super) allowed_paths: Vec<String>,
    pub(super) status: TaskStatus,
    pub(super) checklist: Vec<TaskChecklistItem>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct AgentThread {
    pub(super) messages: Vec<Message>,
}

impl AgentTaskState {
    pub(super) fn new(
        kind: TaskKind,
        task_id: String,
        previous_current_spec_hash: &str,
        target_spec_hash: &str,
        max_steps: usize,
        protected_paths: Vec<String>,
        allowed_paths: Vec<String>,
    ) -> Self {
        Self {
            kind,
            task_id,
            previous_current_spec_hash: previous_current_spec_hash.to_string(),
            target_spec_hash: target_spec_hash.to_string(),
            max_steps,
            protected_paths,
            allowed_paths,
            status: TaskStatus::Running,
            checklist: vec![
                checklist_item("task-created", "Task created"),
                checklist_item("agent-started", "Agent loop started"),
                checklist_item("completed", "Task completed"),
            ],
        }
    }

    pub(super) fn complete_step(&mut self, id: &str) {
        if let Some(item) = self.checklist.iter_mut().find(|item| item.id == id) {
            item.status = TaskStepStatus::Completed;
        }
    }

    pub(super) fn complete_or_add_step(&mut self, id: String, label: String) {
        if let Some(item) = self.checklist.iter_mut().find(|item| item.id == id) {
            item.label = label;
            item.status = TaskStepStatus::Completed;
        } else {
            self.checklist.push(TaskChecklistItem {
                id,
                label,
                status: TaskStepStatus::Completed,
            });
        }
    }
}

fn checklist_item(id: &str, label: &str) -> TaskChecklistItem {
    TaskChecklistItem {
        id: id.to_string(),
        label: label.to_string(),
        status: TaskStepStatus::Pending,
    }
}

pub(super) fn persist_patch_history(task_dir: &Path, patches: &[ProposedPatch]) -> Result<()> {
    fs::write(
        task_dir.join("patches.json"),
        serde_json::to_string_pretty(patches)?,
    )
    .with_context(|| {
        format!(
            "failed to write {}",
            task_dir.join("patches.json").display()
        )
    })?;

    if let Some((index, patch)) = patches.iter().enumerate().last() {
        persist_patch_record(task_dir, index + 1, patch)?;
    }

    Ok(())
}

fn persist_patch_record(task_dir: &Path, index: usize, patch: &ProposedPatch) -> Result<()> {
    let patch_path = task_dir.join(format!("patch-{index}.apply"));
    fs::write(&patch_path, ensure_trailing_newline(&patch.patch))
        .with_context(|| format!("failed to write {}", patch_path.display()))?;
    fs::write(
        task_dir.join(format!("patch-{index}.json")),
        serde_json::to_string_pretty(patch)?,
    )
    .with_context(|| {
        format!(
            "failed to write {}",
            task_dir.join(format!("patch-{index}.json")).display()
        )
    })?;

    Ok(())
}

pub(super) fn write_task_state(task_dir: &Path, state: &AgentTaskState) -> Result<()> {
    fs::write(
        task_dir.join("task.json"),
        serde_json::to_string_pretty(state)?,
    )
    .with_context(|| format!("failed to write {}", task_dir.join("task.json").display()))?;

    Ok(())
}

pub(super) fn read_task_state(task_dir: &Path) -> Result<AgentTaskState> {
    let path = task_dir.join("task.json");
    let source =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&source).with_context(|| format!("failed to parse {}", path.display()))
}

pub(super) fn write_thread(task_dir: &Path, thread: &AgentThread) -> Result<()> {
    fs::write(
        task_dir.join("thread.json"),
        serde_json::to_string_pretty(thread)?,
    )
    .with_context(|| format!("failed to write {}", task_dir.join("thread.json").display()))?;

    Ok(())
}

pub(super) fn read_thread(task_dir: &Path) -> Result<AgentThread> {
    let path = task_dir.join("thread.json");
    let source =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&source).with_context(|| format!("failed to parse {}", path.display()))
}

pub(super) fn read_patch_history(task_dir: &Path) -> Result<Vec<ProposedPatch>> {
    let path = task_dir.join("patches.json");
    if !path.exists() {
        return Ok(Vec::new());
    }

    let source =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&source).with_context(|| format!("failed to parse {}", path.display()))
}

pub(super) fn read_result(task_dir: &Path) -> Result<Option<String>> {
    let path = task_dir.join("result.md");
    if !path.exists() {
        return Ok(None);
    }

    fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))
        .map(Some)
}

pub(super) fn latest_pending_task_dir(kind: TaskKind) -> Result<Option<PathBuf>> {
    let tasks_root = Path::new(TASKS_DIR);
    if !tasks_root.exists() {
        return Ok(None);
    }

    let mut pending = Vec::new();
    for entry in fs::read_dir(tasks_root).context("failed to read .specforge tasks directory")? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let task_dir = entry.path();
        let task_path = task_dir.join("task.json");
        if !task_path.exists() {
            continue;
        }
        let state = read_task_state(&task_dir)?;
        if state.status == TaskStatus::Running && state.kind == kind {
            pending.push(task_dir);
        }
    }
    pending.sort();

    Ok(pending.pop())
}
