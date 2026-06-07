use std::{
    env, fs,
    io::{self, IsTerminal, Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use clap::CommandFactory;
use specforge::{
    agent::{
        DevelopmentAgentOptions, DevelopmentAgentRun, TaskStepStatus, has_pending_code_change_task,
        has_pending_development_task, has_pending_test_coverage_task,
        resume_pending_code_change_task, resume_pending_code_change_task_with_events,
        resume_pending_development_task, resume_pending_development_task_with_events,
        resume_pending_test_coverage_task, resume_pending_test_coverage_task_with_events,
        run_code_change_agent, run_code_change_agent_with_events, run_development_agent,
        run_development_agent_with_events, run_project_checks, run_test_coverage_agent,
        run_test_coverage_agent_with_events,
    },
    assist::{AssistExpandOptions, expand_spec},
    config::{CURRENT_MODEL, CURRENT_SPEC, load_project_config},
    diff::{diff_models, locate_diff_changes},
    init::{InitOptions, init_spec},
    spec::{
        ParsedSpec, Severity, SpecItem, parse_spec, parse_spec_file, print_diagnostics,
        validate_model,
    },
    state::write_current_state,
    sync::{SyncTagOptions, normalize_spec_tags},
};

use crate::cli::{
    args::{AssistCommand, Cli, Command, TestCommand},
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
                    allowed_paths: agent_allowed_paths()?,
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
                        allowed_paths: agent_allowed_paths()?,
                    },
                    no_tui,
                )
                .await?;
                print_agent_run("execution task", &run);
            }
        }
        Command::Fix {
            request,
            images,
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
                    allowed_paths: agent_allowed_paths()?,
                },
                no_tui,
            )
            .await?
            {
                print_agent_run("resumed code change task", &run);
                return Ok(());
            }

            let attachments = read_image_attachments(&images)?;
            let request = read_fix_request(&request, !attachments.is_empty())?;
            let run = run_code_change_agent_with_progress(
                &request,
                &attachments,
                DevelopmentAgentOptions {
                    provider,
                    model,
                    max_steps: agent_steps,
                    protected_paths: Vec::new(),
                    allowed_paths: agent_allowed_paths()?,
                },
                no_tui,
            )
            .await?;
            print_agent_run("code change task", &run);
        }
        Command::Test { command } => match command {
            TestCommand::Run => {
                let checks = run_project_checks()?;
                print_project_check_run(&checks);
                if !checks.success {
                    bail!("project checks failed");
                }
            }
            TestCommand::Cover {
                target,
                files,
                spec_items,
                spec,
                agent_steps,
                provider,
                model,
                no_tui,
            } => {
                if let Some(run) = resume_test_coverage_task_with_progress(
                    DevelopmentAgentOptions {
                        provider,
                        model: model.clone(),
                        max_steps: agent_steps,
                        protected_paths: vec![spec.clone()],
                        allowed_paths: agent_allowed_paths()?,
                    },
                    no_tui,
                )
                .await?
                {
                    print_agent_run("resumed test coverage task", &run);
                    return Ok(());
                }

                let request = build_test_coverage_request(&target, &files, &spec_items, &spec)?;
                let run = run_test_coverage_agent_with_progress(
                    &request,
                    DevelopmentAgentOptions {
                        provider,
                        model,
                        max_steps: agent_steps,
                        protected_paths: vec![spec],
                        allowed_paths: agent_allowed_paths()?,
                    },
                    no_tui,
                )
                .await?;
                print_agent_run("test coverage task", &run);
            }
        },
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
        Command::Completions { shell } => {
            let mut command = Cli::command();
            let bin_name = command.get_name().to_owned();
            clap_complete::generate(shell, &mut command, bin_name, &mut io::stdout());
        }
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

fn print_project_check_run(run: &specforge::agent::ProjectCheckRun) {
    if let Some(reason) = &run.skipped_reason {
        println!("test run: skipped: {reason}");
        return;
    }

    println!("test run: {} check(s)", run.checks.len());
    for check in &run.checks {
        let status = if check.success {
            "ok"
        } else if check.timed_out {
            "timed out"
        } else {
            "failed"
        };
        println!("{status}: {}", check.command.join(" "));
        if !check.success {
            if let Some(exit_code) = check.exit_code {
                println!("  exit code: {exit_code}");
            }
            print_check_output("stdout", &check.stdout_tail);
            print_check_output("stderr", &check.stderr_tail);
        } else if let Some(reason) = &check.skipped_reason {
            println!("  skipped: {reason}");
        }
    }
}

fn print_check_output(label: &str, output: &str) {
    let output = output.trim_end();
    if output.trim().is_empty() {
        return;
    }

    println!("{label}:");
    println!("{output}");
}

fn agent_allowed_paths() -> Result<Vec<String>> {
    Ok(load_project_config()?.file_access.allowed)
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

async fn resume_test_coverage_task_with_progress(
    options: DevelopmentAgentOptions,
    no_tui: bool,
) -> Result<Option<DevelopmentAgentRun>> {
    if !should_use_tui(no_tui) {
        return resume_pending_test_coverage_task(options).await;
    }
    if !has_pending_test_coverage_task()? {
        return Ok(None);
    }

    run_with_tui("SpecForge test cover", |progress| async move {
        progress.log("Looking for a pending test coverage task");
        resume_pending_test_coverage_task_with_events(options, |event| {
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
    images: &[specforge::agent::ImageAttachment],
    options: DevelopmentAgentOptions,
    no_tui: bool,
) -> Result<DevelopmentAgentRun> {
    if !should_use_tui(no_tui) {
        return run_code_change_agent(request, images, options).await;
    }

    run_with_tui("SpecForge fix", |progress| async move {
        progress.log("Starting code change agent");
        run_code_change_agent_with_events(request, images, options, |event| {
            progress.agent_event(event);
        })
        .await
    })
    .await
}

async fn run_test_coverage_agent_with_progress(
    request: &str,
    options: DevelopmentAgentOptions,
    no_tui: bool,
) -> Result<DevelopmentAgentRun> {
    if !should_use_tui(no_tui) {
        return run_test_coverage_agent(request, options).await;
    }

    run_with_tui("SpecForge test cover", |progress| async move {
        progress.log("Starting test coverage agent");
        run_test_coverage_agent_with_events(request, options, |event| {
            progress.agent_event(event);
        })
        .await
    })
    .await
}

fn should_use_tui(no_tui: bool) -> bool {
    !no_tui && io::stdout().is_terminal()
}

fn read_fix_request(parts: &[String], has_images: bool) -> Result<String> {
    let request = parts.join(" ");
    if !request.trim().is_empty() {
        return Ok(request);
    }

    if has_images {
        return Ok(
            "Analyze the attached screenshot(s) and apply the necessary code fix.".to_string(),
        );
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

fn read_image_attachments(paths: &[PathBuf]) -> Result<Vec<specforge::agent::ImageAttachment>> {
    paths
        .iter()
        .enumerate()
        .map(|(index, path)| read_image_attachment(index + 1, path))
        .collect()
}

fn read_image_attachment(index: usize, path: &Path) -> Result<specforge::agent::ImageAttachment> {
    let (name, bytes) = if path == Path::new("-") {
        let mut bytes = Vec::new();
        io::stdin()
            .read_to_end(&mut bytes)
            .context("failed to read image attachment from stdin")?;
        if bytes.is_empty() {
            bail!("image attachment from stdin is empty");
        }
        (format!("stdin-image-{index}"), bytes)
    } else {
        let bytes = fs::read(path)
            .with_context(|| format!("failed to read image attachment {}", path.display()))?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("image")
            .to_string();
        (name, bytes)
    };
    let media_type = image_media_type(path, &bytes)?;

    Ok(specforge::agent::ImageAttachment {
        name,
        media_type,
        bytes,
    })
}

fn image_media_type(path: &Path, bytes: &[u8]) -> Result<rig::message::ImageMediaType> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Ok(rig::message::ImageMediaType::PNG);
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Ok(rig::message::ImageMediaType::JPEG);
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Ok(rig::message::ImageMediaType::GIF);
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Ok(rig::message::ImageMediaType::WEBP);
    }

    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => Ok(rig::message::ImageMediaType::PNG),
        Some("jpg" | "jpeg") => Ok(rig::message::ImageMediaType::JPEG),
        Some("gif") => Ok(rig::message::ImageMediaType::GIF),
        Some("webp") => Ok(rig::message::ImageMediaType::WEBP),
        Some("heic") => Ok(rig::message::ImageMediaType::HEIC),
        Some("heif") => Ok(rig::message::ImageMediaType::HEIF),
        Some("svg") => Ok(rig::message::ImageMediaType::SVG),
        _ => bail!(
            "unsupported image attachment {}; expected PNG, JPEG, GIF, WEBP, HEIC, HEIF, or SVG",
            path.display()
        ),
    }
}

fn build_test_coverage_request(
    target_parts: &[String],
    files: &[PathBuf],
    spec_items: &[String],
    spec: &Path,
) -> Result<String> {
    let structured_target_count = files.len() + spec_items.len();
    let target = read_test_coverage_target(target_parts, structured_target_count > 0)?;
    let resolved_items = resolve_spec_items(spec_items, spec)?;

    let mut request = String::new();
    request.push_str(
        "Add or improve automated test coverage for the requested area.\n\
         Prefer test-only changes. Change production code only when a test reveals a real defect \
         or when a minimal refactor for testability is necessary, and explain that choice in the final \
         answer. Inspect the repository before deciding where tests belong. Use the configured \
         project checks as the verification target.\n",
    );

    if let Some(target) = target {
        request.push_str("\n<coverage-target>\n");
        request.push_str(target.trim());
        request.push_str("\n</coverage-target>\n");
    }

    if !files.is_empty() {
        request.push_str("\n<target-files>\n");
        for file in files {
            request.push_str("- ");
            request.push_str(&file.display().to_string());
            request.push('\n');
        }
        request.push_str("</target-files>\n");
    }

    if !resolved_items.is_empty() {
        request.push_str("\n<spec-items>\n");
        for (query, item, source) in resolved_items {
            request.push_str(&format!(
                "## Query: {query}\n- id: {}\n- kind: {:?}\n- title: {}\n- lines: {}-{}\n\n",
                item.id.as_deref().unwrap_or("<none>"),
                item.kind,
                item.title,
                item.source_range.start_line,
                item.source_range.end_line,
            ));
            request.push_str("```asciidoc\n");
            request.push_str(source.trim());
            request.push_str("\n```\n\n");
        }
        request.push_str("</spec-items>\n");
    }

    Ok(request)
}

fn read_test_coverage_target(
    parts: &[String],
    has_structured_targets: bool,
) -> Result<Option<String>> {
    let target = parts.join(" ");
    if !target.trim().is_empty() {
        return Ok(Some(target));
    }

    if !io::stdin().is_terminal() {
        let mut target = String::new();
        io::stdin()
            .read_to_string(&mut target)
            .context("failed to read test coverage target from stdin")?;
        if !target.trim().is_empty() {
            return Ok(Some(target));
        }
    }

    if has_structured_targets {
        return Ok(None);
    }

    bail!("test cover needs a target argument, --file, --item/--entity, or piped stdin");
}

fn resolve_spec_items(items: &[String], spec: &Path) -> Result<Vec<(String, SpecItem, String)>> {
    if items.is_empty() {
        return Ok(Vec::new());
    }

    let parsed = parse_spec_file(spec)?;
    items
        .iter()
        .map(|query| resolve_spec_item(query, &parsed))
        .collect()
}

fn resolve_spec_item(query: &str, parsed: &ParsedSpec) -> Result<(String, SpecItem, String)> {
    let query = query.trim();
    if query.is_empty() {
        bail!("spec item query must not be empty");
    }

    let matches = parsed
        .model
        .items
        .iter()
        .filter(|item| spec_item_matches(item, query))
        .collect::<Vec<_>>();

    match matches.as_slice() {
        [item] => Ok((
            query.to_string(),
            (*item).clone(),
            spec_item_source(&parsed.source, item),
        )),
        [] => bail!("spec item `{query}` was not found"),
        matches => {
            let labels = matches
                .iter()
                .map(|item| {
                    format!(
                        "{} ({})",
                        item.id.as_deref().unwrap_or("<no id>"),
                        item.title
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            bail!("spec item `{query}` is ambiguous: {labels}");
        }
    }
}

fn spec_item_matches(item: &SpecItem, query: &str) -> bool {
    item.id.as_deref() == Some(query)
        || item.title.eq_ignore_ascii_case(query)
        || item.heading.eq_ignore_ascii_case(query)
}

fn spec_item_source(source: &str, item: &SpecItem) -> String {
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
