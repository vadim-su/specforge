use std::{
    env, fs,
    io::{self, IsTerminal, Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use specforge::{
    agent::{
        DevelopmentAgentOptions, DevelopmentAgentRun, TaskStepStatus, has_pending_code_change_task,
        has_pending_development_task, resume_pending_code_change_task,
        resume_pending_code_change_task_with_events, resume_pending_development_task,
        resume_pending_development_task_with_events, run_code_change_agent,
        run_code_change_agent_with_events, run_development_agent,
        run_development_agent_with_events,
    },
    assist::{AssistExpandOptions, expand_spec},
    config::{CURRENT_MODEL, CURRENT_SPEC},
    diff::{diff_models, locate_diff_changes},
    init::{InitOptions, init_spec},
    spec::{ParsedSpec, Severity, parse_spec, parse_spec_file, print_diagnostics, validate_model},
    state::write_current_state,
    sync::{SyncTagOptions, normalize_spec_tags},
};

use crate::cli::{
    args::{AssistCommand, Cli, Command},
    color::Colors,
    diff_render::{print_diff, print_text_diff},
    tui::run_with_tui,
};

pub async fn run(cli: Cli) -> Result<()> {
    let Cli {
        project_root,
        command,
    } = cli;
    if let Some(project_root) = project_root {
        enter_project_root(&project_root)?;
    }

    match command {
        Command::Init {
            input,
            output,
            force,
            template,
            provider,
            model,
            no_tui,
        } => {
            init_spec(InitOptions {
                input,
                output,
                force,
                template,
                provider,
                model,
                no_tui,
            })
            .await?;
        }
        Command::Check { spec } => {
            let parsed = parse_spec_file(&spec)?;
            let diagnostics = validate_model(&parsed.model);
            print_diagnostics(&diagnostics);

            if diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Error)
            {
                bail!("spec validation failed");
            }

            println!("ok: {} spec items", parsed.model.items.len());
        }
        Command::Model { spec } => {
            let parsed = parse_spec_file(&spec)?;
            println!("{}", serde_json::to_string_pretty(&parsed.model)?);
        }
        Command::Diff { spec, color } => {
            let current = parse_spec_file(&spec)?;
            let (baseline, is_initial_diff) = read_current_state_or_empty()?;

            let diagnostics = validate_model(&current.model);
            if diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Error)
            {
                print_diagnostics(&diagnostics);
                bail!("current spec validation failed");
            }

            let diff = locate_diff_changes(
                &baseline,
                &current,
                diff_models(&baseline.model, &current.model),
            );
            let colors = Colors::new(color);
            if is_initial_diff {
                println!("No current state found; treating the spec as fully new.");
            }
            print_diff(&diff, &colors);
            if !is_initial_diff {
                print_text_diff(&baseline, &current, &diff, &colors);
            }
        }
        Command::Sync {
            spec,
            yes,
            skip_agent,
            agent_steps,
            color,
            provider,
            model,
            no_tui,
        } => {
            if let Some(run) = resume_development_task_with_progress(
                DevelopmentAgentOptions {
                    provider,
                    model: model.clone(),
                    max_steps: agent_steps,
                    protected_paths: vec![spec.clone()],
                },
                no_tui,
            )
            .await?
            {
                print_agent_run("resumed execution task", &run);
                return Ok(());
            }

            let (baseline, is_initial_diff) = read_current_state_or_empty()?;
            let current = parse_spec_file(&spec)?;
            let normalized = normalize_spec_tags(
                &baseline,
                &current,
                SyncTagOptions {
                    provider,
                    model: model.clone(),
                },
            )
            .await?;

            let diagnostics = validate_model(&normalized.model);
            print_diagnostics(&diagnostics);

            if diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Error)
            {
                bail!("normalized spec validation failed; no files were written");
            }

            let diff = locate_diff_changes(
                &baseline,
                &normalized,
                diff_models(&baseline.model, &normalized.model),
            );
            let colors = Colors::new(color);
            if is_initial_diff {
                println!("No current state found; treating the spec as fully new.");
            }
            print_diff(&diff, &colors);
            if !is_initial_diff {
                print_text_diff(&baseline, &normalized, &diff, &colors);
            }

            if diff.added.is_empty() && diff.removed.is_empty() && diff.changed.is_empty() {
                if normalized.source == current.source {
                    println!("sync: no semantic changes to accept");
                    return Ok(());
                }

                println!("sync: only tag/format updates were produced");
                if !yes && !confirm_sync()? {
                    println!("sync cancelled; no files were written");
                    return Ok(());
                }

                fs::write(&spec, &normalized.source)
                    .with_context(|| format!("failed to write {}", spec.display()))?;
                write_current_state(&normalized)?;
                println!("updated tags: {}", spec.display());
                println!("current state updated: {}", spec.display());
                println!("execution agent: skipped; no semantic changes");
                return Ok(());
            }

            if !yes && !confirm_sync()? {
                println!("sync cancelled; no files were written");
                return Ok(());
            }

            if normalized.source != current.source {
                fs::write(&spec, &normalized.source)
                    .with_context(|| format!("failed to write {}", spec.display()))?;
                println!("updated tags: {}", spec.display());
            }
            write_current_state(&normalized)?;

            println!("current state updated: {}", spec.display());
            if skip_agent {
                println!("execution agent: skipped");
            } else {
                let run = run_development_agent_with_progress(
                    &baseline,
                    &normalized,
                    &diff,
                    DevelopmentAgentOptions {
                        provider,
                        model,
                        max_steps: agent_steps,
                        protected_paths: vec![spec.clone()],
                    },
                    no_tui,
                )
                .await?;
                print_agent_run("execution task", &run);
            }
        }
        Command::Fix {
            request,
            agent_steps,
            provider,
            model,
            no_tui,
        } => {
            if let Some(run) = resume_code_change_task_with_progress(
                DevelopmentAgentOptions {
                    provider,
                    model: model.clone(),
                    max_steps: agent_steps,
                    protected_paths: Vec::new(),
                },
                no_tui,
            )
            .await?
            {
                print_agent_run("resumed code change task", &run);
                return Ok(());
            }

            let request = read_fix_request(&request)?;
            let run = run_code_change_agent_with_progress(
                &request,
                DevelopmentAgentOptions {
                    provider,
                    model,
                    max_steps: agent_steps,
                    protected_paths: Vec::new(),
                },
                no_tui,
            )
            .await?;
            print_agent_run("code change task", &run);
        }
        Command::Assist { command } => match command {
            AssistCommand::Expand {
                spec,
                focus,
                provider,
                model,
                no_tui,
            } => {
                let response = expand_spec(AssistExpandOptions {
                    spec,
                    focus,
                    provider,
                    model,
                    no_tui,
                })
                .await?;
                println!("{}", response.trim());
            }
        },
        Command::Accept { spec } => {
            let parsed = parse_spec_file(&spec)?;
            let diagnostics = validate_model(&parsed.model);
            print_diagnostics(&diagnostics);

            if diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Error)
            {
                bail!("spec validation failed; current state was not updated");
            }

            write_current_state(&parsed)?;

            println!("current state updated: {}", spec.display());
            println!("wrote: {CURRENT_SPEC}");
            println!("wrote: {CURRENT_MODEL}");
        }
    }

    Ok(())
}

fn print_agent_run(label: &str, run: &DevelopmentAgentRun) {
    println!("{label}: {}", run.task_dir.display());
    println!("Completed:");
    for item in &run.checklist {
        let mark = match item.status {
            TaskStepStatus::Completed => "x",
            TaskStepStatus::Pending => " ",
        };
        println!("  [{mark}] {}", item.label);
    }
    if !run.final_answer.trim().is_empty() {
        println!("{}", run.final_answer.trim());
    }
}

async fn resume_development_task_with_progress(
    options: DevelopmentAgentOptions,
    no_tui: bool,
) -> Result<Option<DevelopmentAgentRun>> {
    if !should_use_tui(no_tui) {
        return resume_pending_development_task(options).await;
    }
    if !has_pending_development_task()? {
        return Ok(None);
    }

    run_with_tui("SpecForge sync", |progress| async move {
        progress.log("Looking for a pending execution task");
        resume_pending_development_task_with_events(options, |event| {
            progress.agent_event(event);
        })
        .await
    })
    .await
}

async fn resume_code_change_task_with_progress(
    options: DevelopmentAgentOptions,
    no_tui: bool,
) -> Result<Option<DevelopmentAgentRun>> {
    if !should_use_tui(no_tui) {
        return resume_pending_code_change_task(options).await;
    }
    if !has_pending_code_change_task()? {
        return Ok(None);
    }

    run_with_tui("SpecForge fix", |progress| async move {
        progress.log("Looking for a pending code change task");
        resume_pending_code_change_task_with_events(options, |event| {
            progress.agent_event(event);
        })
        .await
    })
    .await
}

async fn run_development_agent_with_progress(
    previous_current: &ParsedSpec,
    target: &ParsedSpec,
    diff: &specforge::diff::ModelDiff,
    options: DevelopmentAgentOptions,
    no_tui: bool,
) -> Result<DevelopmentAgentRun> {
    if !should_use_tui(no_tui) {
        return run_development_agent(previous_current, target, diff, options).await;
    }

    run_with_tui("SpecForge sync", |progress| async move {
        progress.log("Starting execution agent");
        run_development_agent_with_events(previous_current, target, diff, options, |event| {
            progress.agent_event(event);
        })
        .await
    })
    .await
}

async fn run_code_change_agent_with_progress(
    request: &str,
    options: DevelopmentAgentOptions,
    no_tui: bool,
) -> Result<DevelopmentAgentRun> {
    if !should_use_tui(no_tui) {
        return run_code_change_agent(request, options).await;
    }

    run_with_tui("SpecForge fix", |progress| async move {
        progress.log("Starting code change agent");
        run_code_change_agent_with_events(request, options, |event| {
            progress.agent_event(event);
        })
        .await
    })
    .await
}

fn should_use_tui(no_tui: bool) -> bool {
    !no_tui && io::stdout().is_terminal()
}

fn read_fix_request(parts: &[String]) -> Result<String> {
    let request = parts.join(" ");
    if !request.trim().is_empty() {
        return Ok(request);
    }

    if io::stdin().is_terminal() {
        bail!("fix needs a request argument or piped stdin");
    }

    let mut request = String::new();
    io::stdin()
        .read_to_string(&mut request)
        .context("failed to read fix request from stdin")?;

    if request.trim().is_empty() {
        bail!("fix request is empty");
    }

    Ok(request)
}

fn enter_project_root(project_root: &Path) -> Result<()> {
    let root = absolute_path(project_root)?;
    if !root.is_dir() {
        bail!(
            "project root does not exist or is not a directory: {}",
            root.display()
        );
    }

    env::set_current_dir(&root)
        .with_context(|| format!("failed to enter project root {}", root.display()))?;

    Ok(())
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    Ok(env::current_dir()
        .context("failed to read current directory")?
        .join(path))
}

fn read_current_state_or_empty() -> Result<(ParsedSpec, bool)> {
    if Path::new(CURRENT_SPEC).exists() {
        return Ok((parse_spec_file(Path::new(CURRENT_SPEC))?, false));
    }

    Ok((
        ParsedSpec {
            source: String::new(),
            model: parse_spec(""),
        },
        true,
    ))
}

fn confirm_sync() -> Result<bool> {
    if !io::stdin().is_terminal() {
        bail!("sync needs --yes when stdin is not interactive");
    }

    print!("Accept this spec for execution? [y/N] ");
    io::stdout().flush().context("failed to flush stdout")?;

    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .context("failed to read confirmation")?;

    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "YES"))
}
