use std::{
    fmt, fs,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use rig::message::{Message, ToolCall};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    agents::{RigAgentConfig, RigAgentFactory, RuntimeAgent},
    config::TASKS_DIR,
    diff::{ModelDiff, display_item_key},
    prompts,
    provider::Provider,
    spec::{ParsedSpec, SpecItem},
};

mod checks;
mod patch;
mod project_files;
mod tools;

use checks::{project_check_plan, run_project_checks};
use patch::{ProposedPatch, apply_proposed_patch, validate_apply_patch_with_protected_paths};
use project_files::{inspect_file, list_project_files};
use tools::{code_change_tools, development_tools};

const DEFAULT_AGENT_TURN_BUDGET: usize = 32;
const UNBOUNDED_AGENT_TURN_BUDGET: usize = 0;

#[derive(Debug)]
pub struct DevelopmentAgentOptions {
    pub provider: Provider,
    pub model: Option<String>,
    pub max_steps: Option<usize>,
    pub protected_paths: Vec<PathBuf>,
    pub allowed_paths: Vec<String>,
}

#[derive(Debug)]
pub struct DevelopmentAgentRun {
    pub task_dir: PathBuf,
    pub final_answer: String,
    pub checklist: Vec<TaskChecklistItem>,
}

#[derive(Debug, Clone)]
pub enum DevelopmentAgentEvent {
    TaskCreated {
        task_dir: PathBuf,
        checklist: Vec<TaskChecklistItem>,
        max_steps: usize,
    },
    TaskResumed {
        task_dir: PathBuf,
        checklist: Vec<TaskChecklistItem>,
        max_steps: usize,
    },
    ChecklistUpdated(Vec<TaskChecklistItem>),
    StepStarted {
        step: usize,
        max_steps: usize,
    },
    AgentTurnCompleted {
        step: usize,
        tool_calls: Vec<String>,
    },
    ToolStarted {
        name: String,
    },
    ToolFinished {
        name: String,
        ok: Option<bool>,
    },
    Log(String),
    Finished {
        completed: bool,
        checklist: Vec<TaskChecklistItem>,
        final_answer: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskChecklistItem {
    pub id: String,
    pub label: String,
    pub status: TaskStepStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TaskStepStatus {
    Pending,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum TaskStatus {
    Running,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum TaskKind {
    SpecSync,
    CodeChange,
}

fn default_task_kind() -> TaskKind {
    TaskKind::SpecSync
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentTurnBudget {
    Unbounded,
    Limited(usize),
}

impl AgentTurnBudget {
    fn from_cli(value: Option<usize>) -> Self {
        match value {
            Some(UNBOUNDED_AGENT_TURN_BUDGET) => Self::Unbounded,
            Some(value) => Self::Limited(value.max(1)),
            None => Self::Limited(DEFAULT_AGENT_TURN_BUDGET),
        }
    }

    fn from_state(value: usize) -> Self {
        match value {
            UNBOUNDED_AGENT_TURN_BUDGET => Self::Unbounded,
            value => Self::Limited(value.max(1)),
        }
    }

    fn as_state_value(self) -> usize {
        match self {
            Self::Unbounded => UNBOUNDED_AGENT_TURN_BUDGET,
            Self::Limited(value) => value,
        }
    }

    fn exhausted_before_turn(self, turn: usize) -> bool {
        match self {
            Self::Unbounded => false,
            Self::Limited(max_turns) => turn > max_turns,
        }
    }
}

impl fmt::Display for AgentTurnBudget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unbounded => write!(f, "unbounded"),
            Self::Limited(value) => write!(f, "{value}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentTaskState {
    #[serde(default = "default_task_kind")]
    kind: TaskKind,
    task_id: String,
    previous_current_spec_hash: String,
    target_spec_hash: String,
    max_steps: usize,
    #[serde(default)]
    protected_paths: Vec<String>,
    #[serde(default)]
    allowed_paths: Vec<String>,
    status: TaskStatus,
    checklist: Vec<TaskChecklistItem>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AgentThread {
    messages: Vec<Message>,
}

#[derive(Debug, Serialize)]
struct DiffSummary<'a> {
    added: Vec<ItemSummary<'a>>,
    removed: Vec<ItemSummary<'a>>,
    changed: Vec<ChangeSummary<'a>>,
}

#[derive(Debug, Serialize)]
struct ItemSummary<'a> {
    id: String,
    kind: String,
    title: &'a str,
    line: usize,
}

#[derive(Debug, Serialize)]
struct ChangeSummary<'a> {
    id: &'a str,
    kind: String,
    title: &'a str,
    line: usize,
    fields: &'a [&'static str],
}

struct AgentToolContext<'a> {
    target: Option<&'a ParsedSpec>,
    diff: Option<Value>,
    protected_paths: Vec<String>,
    allowed_paths: Vec<String>,
}

impl AgentTaskState {
    fn new(
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

    fn complete_step(&mut self, id: &str) {
        if let Some(item) = self.checklist.iter_mut().find(|item| item.id == id) {
            item.status = TaskStepStatus::Completed;
        }
    }

    fn complete_or_add_step(&mut self, id: String, label: String) {
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

pub async fn run_development_agent(
    previous_current: &ParsedSpec,
    target: &ParsedSpec,
    diff: &ModelDiff,
    options: DevelopmentAgentOptions,
) -> Result<DevelopmentAgentRun> {
    run_development_agent_with_events(previous_current, target, diff, options, |_| {}).await
}

pub async fn run_development_agent_with_events(
    previous_current: &ParsedSpec,
    target: &ParsedSpec,
    diff: &ModelDiff,
    options: DevelopmentAgentOptions,
    mut events: impl FnMut(DevelopmentAgentEvent),
) -> Result<DevelopmentAgentRun> {
    let turn_budget = AgentTurnBudget::from_cli(options.max_steps);
    let protected_paths = normalize_protected_paths(&options.protected_paths)?;
    let allowed_paths = normalize_allowed_paths(&options.allowed_paths)?;
    let agent = RigAgentFactory::new(options.provider, options.model).build(RigAgentConfig {
        name: "specforge_development_agent".to_string(),
        preamble: prompts::DEVELOPMENT_AGENT_SYSTEM.to_string(),
        temperature: Some(0.1),
        max_tokens: None,
        tools: development_tools(),
    })?;
    let task_id = task_id(previous_current, target)?;
    let task_dir = Path::new(TASKS_DIR).join(&task_id);
    fs::create_dir_all(&task_dir)
        .with_context(|| format!("failed to create {}", task_dir.display()))?;
    let diff_value = serde_json::to_value(diff_summary(diff))?;
    let context = AgentToolContext {
        target: Some(target),
        diff: Some(diff_value.clone()),
        protected_paths: protected_paths.clone(),
        allowed_paths: allowed_paths.clone(),
    };
    let mut state = AgentTaskState::new(
        TaskKind::SpecSync,
        task_id,
        &previous_current.model.document.content_hash,
        &target.model.document.content_hash,
        turn_budget.as_state_value(),
        protected_paths,
        allowed_paths,
    );

    let mut thread = AgentThread {
        messages: vec![Message::user(initial_user_message(target, &diff_value)?)],
    };

    write_task_state(&task_dir, &state)?;
    fs::write(task_dir.join("target-spec.adoc"), &target.source).with_context(|| {
        format!(
            "failed to write {}",
            task_dir.join("target-spec.adoc").display()
        )
    })?;
    fs::write(
        task_dir.join("spec-diff.json"),
        serde_json::to_string_pretty(&diff_value)?,
    )
    .with_context(|| {
        format!(
            "failed to write {}",
            task_dir.join("spec-diff.json").display()
        )
    })?;
    write_thread(&task_dir, &thread)?;
    state.complete_step("task-created");
    write_task_state(&task_dir, &state)?;
    events(DevelopmentAgentEvent::TaskCreated {
        task_dir: task_dir.clone(),
        checklist: state.checklist.clone(),
        max_steps: state.max_steps,
    });
    events(DevelopmentAgentEvent::Log(format!(
        "Created execution task {}",
        task_dir.display()
    )));

    let mut final_answer = None;
    let mut patch_history = Vec::new();
    let completed = run_agent_loop(
        &agent,
        &task_dir,
        &mut state,
        &mut thread,
        &context,
        &mut patch_history,
        &mut final_answer,
        &mut events,
    )
    .await?;

    finish_task(&task_dir, &mut state, final_answer.as_deref(), completed)?;
    let final_answer = final_answer.unwrap_or_else(|| {
        "Agent paused after the configured turn budget. Run sync again to resume.".to_string()
    });
    events(DevelopmentAgentEvent::Finished {
        completed,
        checklist: state.checklist.clone(),
        final_answer: final_answer.clone(),
    });

    Ok(DevelopmentAgentRun {
        task_dir,
        final_answer,
        checklist: state.checklist,
    })
}

pub async fn resume_pending_development_task(
    options: DevelopmentAgentOptions,
) -> Result<Option<DevelopmentAgentRun>> {
    resume_pending_development_task_with_events(options, |_| {}).await
}

pub fn has_pending_development_task() -> Result<bool> {
    latest_pending_task_dir(TaskKind::SpecSync).map(|task_dir| task_dir.is_some())
}

pub async fn resume_pending_development_task_with_events(
    options: DevelopmentAgentOptions,
    mut events: impl FnMut(DevelopmentAgentEvent),
) -> Result<Option<DevelopmentAgentRun>> {
    let Some(task_dir) = latest_pending_task_dir(TaskKind::SpecSync)? else {
        return Ok(None);
    };

    let mut state = read_task_state(&task_dir)?;
    let mut thread = read_thread(&task_dir)?;
    let target_source =
        fs::read_to_string(task_dir.join("target-spec.adoc")).with_context(|| {
            format!(
                "failed to read {}",
                task_dir.join("target-spec.adoc").display()
            )
        })?;
    let target = ParsedSpec {
        model: crate::spec::parse_spec(&target_source),
        source: target_source,
    };
    let diff = serde_json::from_str::<Value>(
        &fs::read_to_string(task_dir.join("spec-diff.json")).with_context(|| {
            format!(
                "failed to read {}",
                task_dir.join("spec-diff.json").display()
            )
        })?,
    )?;
    let context = AgentToolContext {
        target: Some(&target),
        diff: Some(diff),
        protected_paths: state.protected_paths.clone(),
        allowed_paths: state.allowed_paths.clone(),
    };
    let agent = RigAgentFactory::new(options.provider, options.model).build(RigAgentConfig {
        name: "specforge_development_agent".to_string(),
        preamble: prompts::DEVELOPMENT_AGENT_SYSTEM.to_string(),
        temperature: Some(0.1),
        max_tokens: None,
        tools: development_tools(),
    })?;
    let mut patch_history = read_patch_history(&task_dir)?;
    let mut final_answer = read_result(&task_dir)?;

    events(DevelopmentAgentEvent::TaskResumed {
        task_dir: task_dir.clone(),
        checklist: state.checklist.clone(),
        max_steps: state.max_steps,
    });
    events(DevelopmentAgentEvent::Log(format!(
        "Resuming execution task {}",
        task_dir.display()
    )));

    if state.status != TaskStatus::Completed {
        let completed = run_agent_loop(
            &agent,
            &task_dir,
            &mut state,
            &mut thread,
            &context,
            &mut patch_history,
            &mut final_answer,
            &mut events,
        )
        .await?;
        finish_task(&task_dir, &mut state, final_answer.as_deref(), completed)?;
        let final_answer_text = final_answer.clone().unwrap_or_else(|| {
            "Task resumed and paused after the configured turn budget.".to_string()
        });
        events(DevelopmentAgentEvent::Finished {
            completed,
            checklist: state.checklist.clone(),
            final_answer: final_answer_text,
        });
    }

    Ok(Some(DevelopmentAgentRun {
        task_dir,
        final_answer: final_answer.unwrap_or_else(|| {
            "Task resumed and paused after the configured turn budget.".to_string()
        }),
        checklist: state.checklist,
    }))
}

pub async fn run_code_change_agent(
    request: &str,
    options: DevelopmentAgentOptions,
) -> Result<DevelopmentAgentRun> {
    run_code_change_agent_with_events(request, options, |_| {}).await
}

pub async fn run_code_change_agent_with_events(
    request: &str,
    options: DevelopmentAgentOptions,
    mut events: impl FnMut(DevelopmentAgentEvent),
) -> Result<DevelopmentAgentRun> {
    let request = request.trim();
    if request.is_empty() {
        bail!("fix request must not be empty");
    }

    let turn_budget = AgentTurnBudget::from_cli(options.max_steps);
    let protected_paths = normalize_protected_paths(&options.protected_paths)?;
    let allowed_paths = normalize_allowed_paths(&options.allowed_paths)?;
    let agent = RigAgentFactory::new(options.provider, options.model).build(RigAgentConfig {
        name: "specforge_code_change_agent".to_string(),
        preamble: prompts::CODE_CHANGE_AGENT_SYSTEM.to_string(),
        temperature: Some(0.1),
        max_tokens: None,
        tools: code_change_tools(),
    })?;
    let request_hash = short_hash(request);
    let task_id = timestamped_task_id("fix", &request_hash)?;
    let task_dir = Path::new(TASKS_DIR).join(&task_id);
    fs::create_dir_all(&task_dir)
        .with_context(|| format!("failed to create {}", task_dir.display()))?;
    let context = AgentToolContext {
        target: None,
        diff: None,
        protected_paths: protected_paths.clone(),
        allowed_paths: allowed_paths.clone(),
    };
    let mut state = AgentTaskState::new(
        TaskKind::CodeChange,
        task_id,
        "",
        &request_hash,
        turn_budget.as_state_value(),
        protected_paths,
        allowed_paths,
    );
    let mut thread = AgentThread {
        messages: vec![Message::user(code_change_user_message(request)?)],
    };

    write_task_state(&task_dir, &state)?;
    fs::write(
        task_dir.join("request.md"),
        ensure_trailing_newline(request),
    )
    .with_context(|| format!("failed to write {}", task_dir.join("request.md").display()))?;
    write_thread(&task_dir, &thread)?;
    state.complete_step("task-created");
    write_task_state(&task_dir, &state)?;
    events(DevelopmentAgentEvent::TaskCreated {
        task_dir: task_dir.clone(),
        checklist: state.checklist.clone(),
        max_steps: state.max_steps,
    });
    events(DevelopmentAgentEvent::Log(format!(
        "Created code change task {}",
        task_dir.display()
    )));

    let mut final_answer = None;
    let mut patch_history = Vec::new();
    let completed = run_agent_loop(
        &agent,
        &task_dir,
        &mut state,
        &mut thread,
        &context,
        &mut patch_history,
        &mut final_answer,
        &mut events,
    )
    .await?;

    finish_task(&task_dir, &mut state, final_answer.as_deref(), completed)?;
    let final_answer = final_answer.unwrap_or_else(|| {
        "Agent paused after the configured turn budget. Run fix again to resume.".to_string()
    });
    events(DevelopmentAgentEvent::Finished {
        completed,
        checklist: state.checklist.clone(),
        final_answer: final_answer.clone(),
    });

    Ok(DevelopmentAgentRun {
        task_dir,
        final_answer,
        checklist: state.checklist,
    })
}

pub fn has_pending_code_change_task() -> Result<bool> {
    latest_pending_task_dir(TaskKind::CodeChange).map(|task_dir| task_dir.is_some())
}

pub async fn resume_pending_code_change_task(
    options: DevelopmentAgentOptions,
) -> Result<Option<DevelopmentAgentRun>> {
    resume_pending_code_change_task_with_events(options, |_| {}).await
}

pub async fn resume_pending_code_change_task_with_events(
    options: DevelopmentAgentOptions,
    mut events: impl FnMut(DevelopmentAgentEvent),
) -> Result<Option<DevelopmentAgentRun>> {
    let Some(task_dir) = latest_pending_task_dir(TaskKind::CodeChange)? else {
        return Ok(None);
    };

    let mut state = read_task_state(&task_dir)?;
    let mut thread = read_thread(&task_dir)?;
    let context = AgentToolContext {
        target: None,
        diff: None,
        protected_paths: state.protected_paths.clone(),
        allowed_paths: state.allowed_paths.clone(),
    };
    let agent = RigAgentFactory::new(options.provider, options.model).build(RigAgentConfig {
        name: "specforge_code_change_agent".to_string(),
        preamble: prompts::CODE_CHANGE_AGENT_SYSTEM.to_string(),
        temperature: Some(0.1),
        max_tokens: None,
        tools: code_change_tools(),
    })?;
    let mut patch_history = read_patch_history(&task_dir)?;
    let mut final_answer = read_result(&task_dir)?;

    events(DevelopmentAgentEvent::TaskResumed {
        task_dir: task_dir.clone(),
        checklist: state.checklist.clone(),
        max_steps: state.max_steps,
    });
    events(DevelopmentAgentEvent::Log(format!(
        "Resuming code change task {}",
        task_dir.display()
    )));

    if state.status != TaskStatus::Completed {
        let completed = run_agent_loop(
            &agent,
            &task_dir,
            &mut state,
            &mut thread,
            &context,
            &mut patch_history,
            &mut final_answer,
            &mut events,
        )
        .await?;
        finish_task(&task_dir, &mut state, final_answer.as_deref(), completed)?;
        let final_answer_text = final_answer.clone().unwrap_or_else(|| {
            "Task resumed and paused after the configured turn budget.".to_string()
        });
        events(DevelopmentAgentEvent::Finished {
            completed,
            checklist: state.checklist.clone(),
            final_answer: final_answer_text,
        });
    }

    Ok(Some(DevelopmentAgentRun {
        task_dir,
        final_answer: final_answer.unwrap_or_else(|| {
            "Task resumed and paused after the configured turn budget.".to_string()
        }),
        checklist: state.checklist,
    }))
}

async fn run_agent_loop(
    agent: &RuntimeAgent,
    task_dir: &Path,
    state: &mut AgentTaskState,
    thread: &mut AgentThread,
    context: &AgentToolContext<'_>,
    patch_history: &mut Vec<ProposedPatch>,
    final_answer: &mut Option<String>,
    events: &mut impl FnMut(DevelopmentAgentEvent),
) -> Result<bool> {
    state.complete_step("agent-started");
    write_task_state(task_dir, state)?;
    events(DevelopmentAgentEvent::ChecklistUpdated(
        state.checklist.clone(),
    ));
    events(DevelopmentAgentEvent::Log("Agent loop started".to_string()));

    let turn_budget = AgentTurnBudget::from_state(state.max_steps);
    let mut step = 1;
    loop {
        if turn_budget.exhausted_before_turn(step) {
            break;
        }
        events(DevelopmentAgentEvent::StepStarted {
            step,
            max_steps: state.max_steps,
        });
        events(DevelopmentAgentEvent::Log(format!(
            "Turn {step}: requesting model turn"
        )));
        let Some(prompt) = thread.messages.pop() else {
            bail!("agent thread has no prompt message");
        };
        let history = thread.messages.clone();
        let turn = agent.turn(prompt.clone(), history).await?;
        let tool_calls = turn.tool_calls.clone();
        let tool_call_names = tool_calls
            .iter()
            .map(|call| call.function.name.clone())
            .collect::<Vec<_>>();
        events(DevelopmentAgentEvent::AgentTurnCompleted {
            step,
            tool_calls: tool_call_names.clone(),
        });

        thread.messages.push(prompt);
        thread.messages.push(turn.assistant_message);
        write_thread(task_dir, thread)?;

        if tool_calls.is_empty() {
            *final_answer = Some(turn.text);
            events(DevelopmentAgentEvent::Log(format!(
                "Turn {step}: final response received"
            )));
            return Ok(true);
        }

        events(DevelopmentAgentEvent::Log(format!(
            "Turn {step}: tool calls: {}",
            tool_call_names.join(", ")
        )));
        for call in &tool_calls {
            events(DevelopmentAgentEvent::ToolStarted {
                name: call.function.name.clone(),
            });
            let output = execute_tool(call, task_dir, state, context, patch_history, events)?;
            events(DevelopmentAgentEvent::ToolFinished {
                name: call.function.name.clone(),
                ok: output.get("ok").and_then(Value::as_bool),
            });
            let output_text =
                serde_json::to_string(&output).context("failed to serialize tool output")?;
            thread.messages.push(Message::tool_result_with_call_id(
                call.id.clone(),
                call.call_id.clone(),
                output_text,
            ));
            write_thread(task_dir, thread)?;
            if call.function.name == "propose_patch" {
                break;
            }
        }

        step += 1;
    }

    if final_answer.is_none() {
        *final_answer = Some(
            "Agent paused after the configured turn budget before producing a final response."
                .to_string(),
        );
    }

    Ok(false)
}

fn finish_task(
    task_dir: &Path,
    state: &mut AgentTaskState,
    final_answer: Option<&str>,
    completed: bool,
) -> Result<()> {
    if let Some(final_answer) = final_answer {
        fs::write(
            task_dir.join("result.md"),
            ensure_trailing_newline(final_answer),
        )
        .with_context(|| format!("failed to write {}", task_dir.join("result.md").display()))?;
    }
    if completed {
        state.complete_step("completed");
        state.status = TaskStatus::Completed;
    }
    write_task_state(task_dir, state)?;

    Ok(())
}

fn persist_patch_history(task_dir: &Path, patches: &[ProposedPatch]) -> Result<()> {
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

fn write_task_state(task_dir: &Path, state: &AgentTaskState) -> Result<()> {
    fs::write(
        task_dir.join("task.json"),
        serde_json::to_string_pretty(state)?,
    )
    .with_context(|| format!("failed to write {}", task_dir.join("task.json").display()))?;

    Ok(())
}

fn read_task_state(task_dir: &Path) -> Result<AgentTaskState> {
    let path = task_dir.join("task.json");
    let source =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&source).with_context(|| format!("failed to parse {}", path.display()))
}

fn write_thread(task_dir: &Path, thread: &AgentThread) -> Result<()> {
    fs::write(
        task_dir.join("thread.json"),
        serde_json::to_string_pretty(thread)?,
    )
    .with_context(|| format!("failed to write {}", task_dir.join("thread.json").display()))?;

    Ok(())
}

fn read_thread(task_dir: &Path) -> Result<AgentThread> {
    let path = task_dir.join("thread.json");
    let source =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&source).with_context(|| format!("failed to parse {}", path.display()))
}

fn read_patch_history(task_dir: &Path) -> Result<Vec<ProposedPatch>> {
    let path = task_dir.join("patches.json");
    if !path.exists() {
        return Ok(Vec::new());
    }

    let source =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&source).with_context(|| format!("failed to parse {}", path.display()))
}

fn read_result(task_dir: &Path) -> Result<Option<String>> {
    let path = task_dir.join("result.md");
    if !path.exists() {
        return Ok(None);
    }

    fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))
        .map(Some)
}

fn latest_pending_task_dir(kind: TaskKind) -> Result<Option<PathBuf>> {
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

fn code_change_user_message(request: &str) -> Result<String> {
    let check_plan = project_check_plan()?;
    Ok(format!(
        "Apply this ad-hoc code change request. Inspect the repository before making repo-specific claims. Prepare a concise verification plan before proposing patches.\n\n<user-request>\n{}\n</user-request>\n\n<project-check-plan>\n{}\n</project-check-plan>",
        request.trim(),
        serde_json::to_string_pretty(&check_plan)?
    ))
}

fn initial_user_message(target: &ParsedSpec, diff: &Value) -> Result<String> {
    let check_plan = project_check_plan()?;
    Ok(format!(
        "The spec has been stored as the new current state. Build a repo-specific implementation plan, using tools before making repo-specific claims. Prepare a concise verification plan before proposing patches.\n\n<target-spec-model>\n{}\n</target-spec-model>\n\n<semantic-diff>\n{}\n</semantic-diff>\n\n<project-check-plan>\n{}\n</project-check-plan>",
        serde_json::to_string_pretty(&target.model)?,
        serde_json::to_string_pretty(diff)?,
        serde_json::to_string_pretty(&check_plan)?
    ))
}

fn execute_tool(
    call: &ToolCall,
    task_dir: &Path,
    state: &mut AgentTaskState,
    context: &AgentToolContext<'_>,
    patch_history: &mut Vec<ProposedPatch>,
    events: &mut impl FnMut(DevelopmentAgentEvent),
) -> Result<Value> {
    match call.function.name.as_str() {
        "inspect_spec_diff" => Ok(match &context.diff {
            Some(diff) => json!({
                "ok": true,
                "diff": diff,
            }),
            None => json!({
                "ok": false,
                "error": "spec diff is not available for this task",
            }),
        }),
        "inspect_spec_item" => match context.target {
            Some(target) => inspect_spec_item(call, target),
            None => Ok(json!({
                "ok": false,
                "error": "spec items are not available for this task",
            })),
        },
        "list_project_files" => {
            list_project_files(call, &context.protected_paths, &context.allowed_paths)
        }
        "inspect_file" => inspect_file(call, &context.protected_paths, &context.allowed_paths),
        "propose_patch" => apply_patch_tool(call, task_dir, state, context, patch_history, events),
        other => Ok(json!({
            "ok": false,
            "error": format!("unknown tool `{other}`"),
        })),
    }
}

fn inspect_spec_item(call: &ToolCall, target: &ParsedSpec) -> Result<Value> {
    let Some(id) = call.function.arguments.get("id").and_then(Value::as_str) else {
        return Ok(json!({
            "ok": false,
            "error": "missing string argument `id`",
        }));
    };

    let item = target.model.items.iter().find(|item| {
        item.id
            .as_ref()
            .map(|item_id| item_id == id)
            .unwrap_or(false)
    });

    Ok(match item {
        Some(item) => json!({
            "ok": true,
            "item": item,
            "source": section_source(&target.source, item),
        }),
        None => json!({
            "ok": false,
            "error": format!("spec item `{id}` was not found"),
        }),
    })
}

fn build_proposed_patch(
    call: &ToolCall,
    protected_paths: &[String],
) -> std::result::Result<ProposedPatch, String> {
    let summary = call
        .function
        .arguments
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let Some(patch) = call.function.arguments.get("patch").and_then(Value::as_str) else {
        return Err("missing string argument `patch`".to_string());
    };

    let changes = validate_apply_patch_with_protected_paths(patch, protected_paths)?;
    validate_patch_file_state(&changes)?;

    Ok(ProposedPatch {
        summary,
        patch: ensure_trailing_newline(patch.trim()),
        changes,
        check: None,
    })
}

fn validate_patch_file_state(
    changes: &[patch::PatchFileChange],
) -> std::result::Result<(), String> {
    for change in changes {
        if change.operation == patch::PatchOperation::Add && Path::new(&change.path).exists() {
            return Err(format!(
                "cannot add {}; file already exists; use `*** Update File:` for existing files",
                change.path
            ));
        }
    }

    Ok(())
}

fn apply_patch_tool(
    call: &ToolCall,
    task_dir: &Path,
    state: &mut AgentTaskState,
    context: &AgentToolContext<'_>,
    patch_history: &mut Vec<ProposedPatch>,
    events: &mut impl FnMut(DevelopmentAgentEvent),
) -> Result<Value> {
    let patch_index = patch_history.len() + 1;
    let mut proposed_patch = match build_proposed_patch(call, &context.protected_paths) {
        Ok(patch) => patch,
        Err(error) => {
            return Ok(json!({
                "ok": false,
                "error": error,
            }));
        }
    };

    state.complete_or_add_step(
        format!("patch-{patch_index}-generated"),
        format!("Patch {patch_index} generated"),
    );
    write_task_state(task_dir, state)?;
    events(DevelopmentAgentEvent::ChecklistUpdated(
        state.checklist.clone(),
    ));
    events(DevelopmentAgentEvent::Log(format!(
        "Patch {patch_index} generated: {}",
        proposed_patch.summary
    )));

    if let Err(error) = apply_proposed_patch(&proposed_patch, &context.protected_paths) {
        state.complete_or_add_step(
            format!("patch-{patch_index}-failed"),
            format!("Patch {patch_index} failed to apply"),
        );
        write_task_state(task_dir, state)?;
        events(DevelopmentAgentEvent::ChecklistUpdated(
            state.checklist.clone(),
        ));
        events(DevelopmentAgentEvent::Log(format!(
            "Patch {patch_index} failed to apply: {error}"
        )));
        return Ok(json!({
            "ok": false,
            "applied": false,
            "summary": proposed_patch.summary,
            "changes": proposed_patch.changes,
            "error": error.to_string(),
        }));
    }

    state.complete_or_add_step(
        format!("patch-{patch_index}-applied"),
        format!("Patch {patch_index} applied"),
    );
    write_task_state(task_dir, state)?;
    events(DevelopmentAgentEvent::ChecklistUpdated(
        state.checklist.clone(),
    ));
    events(DevelopmentAgentEvent::Log(format!(
        "Patch {patch_index} applied"
    )));

    events(DevelopmentAgentEvent::Log(format!(
        "Running project checks for patch {patch_index}"
    )));
    let check = run_project_checks()?;
    let check_label = if check.success {
        format!("Checks {patch_index} passed")
    } else if check.skipped_reason.is_some() {
        format!("Checks {patch_index} skipped")
    } else {
        format!("Checks {patch_index} failed")
    };
    state.complete_or_add_step(format!("checks-{patch_index}"), check_label);
    write_task_state(task_dir, state)?;
    events(DevelopmentAgentEvent::ChecklistUpdated(
        state.checklist.clone(),
    ));
    events(DevelopmentAgentEvent::Log(if check.success {
        format!("Checks {patch_index} passed")
    } else if let Some(reason) = &check.skipped_reason {
        format!("Checks {patch_index} skipped: {reason}")
    } else {
        format!("Checks {patch_index} failed")
    }));

    proposed_patch.check = Some(check.clone());
    patch_history.push(proposed_patch.clone());
    persist_patch_history(task_dir, patch_history)?;

    Ok(json!({
        "ok": true,
        "applied": true,
        "summary": proposed_patch.summary,
        "changes": proposed_patch.changes,
        "check": check,
    }))
}

fn diff_summary(diff: &ModelDiff) -> DiffSummary<'_> {
    DiffSummary {
        added: diff.added.iter().map(item_summary).collect(),
        removed: diff.removed.iter().map(item_summary).collect(),
        changed: diff
            .changed
            .iter()
            .map(|change| ChangeSummary {
                id: &change.id,
                kind: format!("{:?}", change.kind),
                title: &change.title,
                line: change.line,
                fields: &change.fields,
            })
            .collect(),
    }
}

fn item_summary(item: &SpecItem) -> ItemSummary<'_> {
    ItemSummary {
        id: display_item_key(item),
        kind: format!("{:?}", item.kind),
        title: &item.title,
        line: item.source_range.start_line,
    }
}

fn section_source(source: &str, item: &SpecItem) -> String {
    source
        .lines()
        .skip(item.source_range.start_line.saturating_sub(1))
        .take(
            item.source_range
                .end_line
                .saturating_sub(item.source_range.start_line)
                + 1,
        )
        .collect::<Vec<_>>()
        .join("\n")
}

fn task_id(previous_current: &ParsedSpec, target: &ParsedSpec) -> Result<String> {
    let seed = format!(
        "{}\n{}",
        previous_current.model.document.content_hash, target.model.document.content_hash
    );
    timestamped_task_id("sync", &seed)
}

fn timestamped_task_id(prefix: &str, seed: &str) -> Result<String> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time is before UNIX_EPOCH")?
        .as_secs();
    let mut hasher = Sha256::new();
    hasher.update(seed.as_bytes());
    hasher.update(timestamp.to_string().as_bytes());
    let digest = hasher.finalize();
    let short_hash = digest
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();

    Ok(format!("{prefix}-{timestamp}-{short_hash}"))
}

fn short_hash(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    hasher
        .finalize()
        .iter()
        .take(12)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn is_safe_relative_path(path: &Path) -> bool {
    !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

fn normalize_protected_paths(paths: &[PathBuf]) -> Result<Vec<String>> {
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

fn normalize_allowed_paths(paths: &[String]) -> Result<Vec<String>> {
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

fn is_specforge_owned_path(path: &Path, protected_paths: &[String]) -> bool {
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

fn ensure_trailing_newline(text: &str) -> String {
    if text.ends_with('\n') {
        text.to_string()
    } else {
        format!("{text}\n")
    }
}

#[cfg(test)]
mod tests {
    use super::patch::{
        PatchFileChange, PatchOperation, validate_apply_patch,
        validate_apply_patch_with_protected_paths,
    };
    use super::*;

    #[test]
    fn rejects_paths_that_escape_repo() {
        assert!(is_safe_relative_path(Path::new("src/lib.rs")));
        assert!(!is_safe_relative_path(Path::new("../secret")));
        assert!(!is_safe_relative_path(Path::new("/tmp/secret")));
    }

    #[test]
    fn validates_apply_patch_paths() {
        let patch = "*** Begin Patch\n*** Add File: src/main.rs\n+fn main() {}\n*** End Patch\n";

        assert_eq!(
            validate_apply_patch(patch).unwrap(),
            vec![PatchFileChange {
                operation: PatchOperation::Add,
                path: "src/main.rs".to_string(),
            }]
        );
    }

    #[test]
    fn rejects_add_file_patch_when_file_already_exists() {
        let changes = vec![PatchFileChange {
            operation: PatchOperation::Add,
            path: "Cargo.toml".to_string(),
        }];

        assert!(validate_patch_file_state(&changes).is_err());
    }

    #[test]
    fn allows_add_file_patch_for_new_file() {
        let changes = vec![PatchFileChange {
            operation: PatchOperation::Add,
            path: "src/nonexistent_generated_file_for_test.rs".to_string(),
        }];

        assert!(validate_patch_file_state(&changes).is_ok());
    }

    #[test]
    fn rejects_apply_patch_paths_that_escape_repo() {
        let patch = "*** Begin Patch\n*** Add File: ../main.rs\n+fn main() {}\n*** End Patch\n";

        assert!(validate_apply_patch(patch).is_err());
    }

    #[test]
    fn rejects_apply_patch_for_default_spec_file() {
        let patch = "*** Begin Patch\n*** Update File: spec.adoc\n@@\n-old\n+new\n*** End Patch\n";

        assert!(validate_apply_patch(patch).is_err());
    }

    #[test]
    fn rejects_apply_patch_for_named_spec_file() {
        let patch =
            "*** Begin Patch\n*** Add File: examples/todoapp.spec.adoc\n+= Todo\n*** End Patch\n";

        assert!(validate_apply_patch(patch).is_err());
    }

    #[test]
    fn rejects_apply_patch_for_specforge_state() {
        let patch =
            "*** Begin Patch\n*** Delete File: .specforge/state/current.adoc\n*** End Patch\n";

        assert!(validate_apply_patch(patch).is_err());
    }

    #[test]
    fn rejects_apply_patch_for_protected_sync_spec_path() {
        let patch =
            "*** Begin Patch\n*** Update File: docs/product.adoc\n@@\n-old\n+new\n*** End Patch\n";

        assert!(
            validate_apply_patch_with_protected_paths(patch, &["docs/product.adoc".to_string()])
                .is_err()
        );
    }

    #[test]
    fn task_state_without_kind_defaults_to_spec_sync() {
        let state = serde_json::from_str::<AgentTaskState>(
            r#"{
                "task_id": "old-task",
                "previous_current_spec_hash": "before",
                "target_spec_hash": "after",
                "max_steps": 6,
                "protected_paths": [],
                "status": "running",
                "checklist": []
            }"#,
        )
        .unwrap();

        assert_eq!(state.kind, TaskKind::SpecSync);
        assert!(state.allowed_paths.is_empty());
    }

    #[test]
    fn normalizes_allowed_file_access_paths() {
        assert_eq!(
            normalize_allowed_paths(&[
                "Cargo.toml".to_string(),
                "./README.md".to_string(),
                "src/".to_string(),
                "examples/**".to_string(),
            ])
            .unwrap(),
            vec![
                "Cargo.toml".to_string(),
                "README.md".to_string(),
                "src/".to_string(),
                "examples/".to_string(),
            ]
        );
    }

    #[test]
    fn rejects_allowed_file_access_paths_that_escape_repo() {
        assert!(normalize_allowed_paths(&["../secret.txt".to_string()]).is_err());
        assert!(normalize_allowed_paths(&["/tmp/secret.txt".to_string()]).is_err());
    }

    #[test]
    fn agent_turn_budget_uses_default_when_unspecified() {
        assert_eq!(
            AgentTurnBudget::from_cli(None),
            AgentTurnBudget::Limited(DEFAULT_AGENT_TURN_BUDGET)
        );
    }

    #[test]
    fn agent_turn_budget_preserves_positive_cli_value() {
        assert_eq!(
            AgentTurnBudget::from_cli(Some(12)),
            AgentTurnBudget::Limited(12)
        );
    }

    #[test]
    fn agent_turn_budget_allows_unbounded_cli_value() {
        let budget = AgentTurnBudget::from_cli(Some(0));

        assert_eq!(budget, AgentTurnBudget::Unbounded);
        assert_eq!(budget.as_state_value(), 0);
        assert!(!budget.exhausted_before_turn(usize::MAX));
    }
}
