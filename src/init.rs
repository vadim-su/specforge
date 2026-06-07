use std::{
    fs,
    io::{self, IsTerminal, Read},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use ratatui::{
    Frame,
    crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};
use serde::Deserialize;

use crate::{
    config::{
        CURRENT_MODEL, CURRENT_SPEC, ProjectCheckConfig, ProjectConfig, clear_project_config,
        project_config_path, write_project_config,
    },
    llm::{LlmClient, LlmPrompt, strip_code_fence},
    prompts,
    provider::Provider,
    spec::{ParsedSpec, Severity, parse_spec, print_diagnostics, validate_model},
    state::clear_current_state,
};

#[derive(Debug)]
pub struct InitOptions {
    pub input: Option<PathBuf>,
    pub output: PathBuf,
    pub force: bool,
    pub template: bool,
    pub provider: Provider,
    pub model: Option<String>,
    pub no_tui: bool,
}

pub async fn init_spec(options: InitOptions) -> Result<()> {
    ensure_init_can_write(&options.output, options.force)?;
    if options.force {
        clear_current_state()?;
        clear_project_config()?;
    }

    let (source, generated_config) = if options.template {
        (starter_template(&options.output), ProjectConfig::default())
    } else {
        let prose = read_init_input(options.input.as_deref())?;
        let client = LlmClient::new(options.provider, options.model);
        let preferences = collect_init_preferences(&client, &prose, options.no_tui).await?;
        let generated = client
            .complete(LlmPrompt {
                system: prompts::INIT_SPEC_SYSTEM.to_string(),
                user: init_user_prompt(&prose, &preferences),
                temperature: Some(0.2),
            })
            .await?;
        let source = strip_code_fence(&generated);
        let generated_config = generate_project_config(&client, &prose, &preferences).await?;
        (source, generated_config)
    };

    let parsed = ParsedSpec {
        model: parse_spec(&source),
        source,
    };
    let diagnostics = validate_model(&parsed.model);
    print_diagnostics(&diagnostics);

    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        bail!("generated spec validation failed; no files were written");
    }

    fs::write(&options.output, &parsed.source)
        .with_context(|| format!("failed to write {}", options.output.display()))?;
    write_generated_project_config(&generated_config)?;

    println!("initialized: {}", options.output.display());
    if generated_config.checks.is_empty() {
        println!("project checks: not configured");
    } else {
        println!("project checks: {}", project_config_path().display());
    }
    println!(
        "current state: not created yet; run `specforge sync {}`",
        options.output.display()
    );

    Ok(())
}

#[derive(Debug, Clone, Default)]
pub struct InitPreferences {
    answers: Vec<InitPreferenceAnswer>,
}

impl InitPreferences {
    fn is_empty(&self) -> bool {
        self.answers.is_empty()
    }

    fn prompt_block(&self) -> String {
        if self.is_empty() {
            return "No additional preferences were provided.".to_string();
        }

        self.answers
            .iter()
            .map(|answer| format!("{}: {}", answer.label, answer.value))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Debug, Clone)]
struct InitPreferenceAnswer {
    label: String,
    value: String,
}

#[derive(Debug, Clone, Deserialize)]
struct InitQuestionnairePlan {
    #[serde(default)]
    questions: Vec<InitQuestion>,
}

#[derive(Debug, Clone, Deserialize)]
struct InitQuestion {
    #[serde(default)]
    label: String,
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    options: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct InitChecksPlan {
    #[serde(default)]
    checks: Vec<InitCheckPlanItem>,
}

#[derive(Debug, Clone, Deserialize)]
struct InitCheckPlanItem {
    #[serde(default)]
    command: Vec<String>,
    timeout_seconds: Option<u64>,
}

#[derive(Debug)]
struct InitQuestionnaire {
    questions: Vec<InitQuestion>,
    answers: InitPreferences,
    current: usize,
    selected: usize,
    custom_input: String,
    editing_custom: bool,
    done: bool,
}

impl InitQuestionnaire {
    fn new(questions: Vec<InitQuestion>) -> Self {
        Self {
            questions,
            answers: InitPreferences::default(),
            current: 0,
            selected: 0,
            custom_input: String::new(),
            editing_custom: false,
            done: false,
        }
    }

    fn active_question(&self) -> &InitQuestion {
        &self.questions[self.current]
    }

    fn select_next(&mut self) {
        if self.editing_custom {
            return;
        }

        let option_count = self.active_question().options.len() + 1;
        self.selected = (self.selected + 1).min(option_count.saturating_sub(1));
    }

    fn select_previous(&mut self) {
        if self.editing_custom {
            return;
        }

        self.selected = self.selected.saturating_sub(1);
    }

    fn accept_selected(&mut self) {
        if self.editing_custom {
            let value = self.custom_input.trim().to_string();
            if !value.is_empty() {
                self.accept_answer(value);
            }
            return;
        }

        let question = self.active_question();
        if self.selected == question.options.len() {
            self.editing_custom = true;
            return;
        }

        self.accept_answer(question.options[self.selected].clone());
    }

    fn skip_current(&mut self) {
        if self.editing_custom {
            self.editing_custom = false;
            self.custom_input.clear();
            return;
        }

        self.advance();
    }

    fn finish(&mut self) {
        self.done = true;
    }

    fn push_char(&mut self, ch: char) {
        if self.editing_custom {
            self.custom_input.push(ch);
        }
    }

    fn backspace(&mut self) {
        if self.editing_custom {
            self.custom_input.pop();
        }
    }

    fn accept_answer(&mut self, value: String) {
        let label = self.active_question().label.clone();
        self.answers
            .answers
            .push(InitPreferenceAnswer { label, value });

        self.advance();
    }

    fn advance(&mut self) {
        self.editing_custom = false;
        self.custom_input.clear();
        self.selected = 0;

        if self.current + 1 >= self.questions.len() {
            self.done = true;
        } else {
            self.current += 1;
        }
    }
}

fn ensure_init_can_write(output: &Path, force: bool) -> Result<()> {
    if !force && output.exists() {
        bail!(
            "{} already exists; pass --force to overwrite it",
            output.display()
        );
    }

    if !force && (Path::new(CURRENT_SPEC).exists() || Path::new(CURRENT_MODEL).exists()) {
        bail!(".specforge current state already exists; pass --force to reinitialize it");
    }

    let config_path = project_config_path();
    if !force && config_path.exists() {
        bail!(
            "{} already exists; pass --force to overwrite it",
            config_path.display()
        );
    }

    Ok(())
}

fn read_init_input(input: Option<&Path>) -> Result<String> {
    let mut prose = String::new();

    if let Some(input) = input {
        prose = fs::read_to_string(input)
            .with_context(|| format!("failed to read init input {}", input.display()))?;
    } else if !io::stdin().is_terminal() {
        io::stdin()
            .read_to_string(&mut prose)
            .context("failed to read init input from stdin")?;
    } else {
        bail!("init needs an input file, piped stdin, or --template");
    }

    if prose.trim().is_empty() {
        bail!("init input is empty");
    }

    Ok(prose)
}

async fn collect_init_preferences(
    client: &LlmClient,
    prose: &str,
    no_tui: bool,
) -> Result<InitPreferences> {
    if no_tui || !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Ok(InitPreferences::default());
    }

    let plan = init_questionnaire_plan(client, prose).await?;
    if plan.questions.is_empty() {
        return Ok(InitPreferences::default());
    }

    run_init_questionnaire(plan.questions)
}

async fn init_questionnaire_plan(client: &LlmClient, prose: &str) -> Result<InitQuestionnairePlan> {
    let response = client
        .complete(LlmPrompt {
            system: init_questionnaire_system_prompt(),
            user: init_questionnaire_user_prompt(prose),
            temperature: Some(0.1),
        })
        .await?;

    let plan = parse_init_questionnaire_plan(&response)?;

    Ok(InitQuestionnairePlan {
        questions: plan
            .questions
            .into_iter()
            .filter_map(normalize_init_question)
            .take(3)
            .collect(),
    })
}

fn normalize_init_question(mut question: InitQuestion) -> Option<InitQuestion> {
    question.label = question.label.trim().to_string();
    question.prompt = question.prompt.trim().to_string();
    question.options = question
        .options
        .into_iter()
        .map(|option| option.trim().to_string())
        .filter(|option| !option.is_empty() && !option.eq_ignore_ascii_case("custom"))
        .take(5)
        .collect();

    if question.label.is_empty() || question.prompt.is_empty() || question.options.is_empty() {
        return None;
    }

    Some(question)
}

fn parse_init_questionnaire_plan(response: &str) -> Result<InitQuestionnairePlan> {
    serde_json::from_str(strip_json_fence(response))
        .context("failed to parse init questionnaire JSON from LLM")
}

async fn generate_project_config(
    client: &LlmClient,
    prose: &str,
    preferences: &InitPreferences,
) -> Result<ProjectConfig> {
    let response = client
        .complete(LlmPrompt {
            system: init_checks_system_prompt(),
            user: init_checks_user_prompt(prose, preferences),
            temperature: Some(0.1),
        })
        .await?;

    let plan = parse_init_checks_plan(&response)?;
    Ok(ProjectConfig {
        file_access: Default::default(),
        integrations: Default::default(),
        checks: plan
            .checks
            .into_iter()
            .filter_map(normalize_init_check)
            .take(5)
            .collect(),
    })
}

fn parse_init_checks_plan(response: &str) -> Result<InitChecksPlan> {
    serde_json::from_str(strip_json_fence(response))
        .context("failed to parse init checks JSON from LLM")
}

fn normalize_init_check(check: InitCheckPlanItem) -> Option<ProjectCheckConfig> {
    let timeout_seconds = check.timeout_seconds.filter(|timeout| *timeout > 0)?;
    let command = check
        .command
        .into_iter()
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if command.is_empty() {
        return None;
    }

    Some(ProjectCheckConfig {
        command,
        timeout_seconds,
    })
}

fn write_generated_project_config(config: &ProjectConfig) -> Result<()> {
    if config.checks.is_empty() {
        return Ok(());
    }

    write_project_config(config)
}

fn init_checks_system_prompt() -> String {
    r#"You create a project verification config for SpecForge.

Return only JSON. Do not wrap it in Markdown fences. Do not include commentary.

Infer the most appropriate local verification commands from the project idea and
user-selected implementation preferences. Choose commands a developer can run
from the repository root after code changes.

Rules:
- Return one or more checks when the project has multiple useful validation
  steps, such as format, lint, typecheck, test, or build.
- Return only checks that are relevant to the inferred stack.
- Prefer standard test commands over broad build commands.
- Include command arguments as separate array items.
- Pick a practical timeout_seconds value for each command based on expected
  project size and stack. Use a positive integer.
- If the stack is unknown or no useful local check can be inferred, return
  {"checks":[]}.

JSON schema:
{
  "checks": [
    {
      "command": ["cargo", "test", "--color", "never"],
      "timeout_seconds": 120
    },
    {
      "command": ["cargo", "fmt", "--check"],
      "timeout_seconds": 30
    }
  ]
}"#
    .to_string()
}

fn init_checks_user_prompt(prose: &str, preferences: &InitPreferences) -> String {
    format!(
        "Create the SpecForge project verification config for this project idea:\n\n<project-idea>\n{}\n</project-idea>\n\nUse these user-selected implementation preferences when they are provided:\n\n<init-preferences>\n{}\n</init-preferences>\n",
        prose.trim(),
        preferences.prompt_block()
    )
}

fn init_questionnaire_system_prompt() -> String {
    r#"You create a small interactive init questionnaire for SpecForge.

Return only JSON. Do not wrap it in Markdown fences. Do not include commentary.

Inspect the user's markdown/prose project idea and decide whether it already specifies:
- programming language or stack
- architecture
- project structure/layout

Ask only about fields that are absent or ambiguous. Do not ask for details already specified.
Return at most one question per field and at most three questions total.
Each question must offer 2 to 5 concise, project-appropriate options.
Do not include a custom/free-form option; the TUI adds that itself.

JSON schema:
{
  "questions": [
    {
      "label": "Programming language",
      "prompt": "Choose what the application should be built with.",
      "options": ["TypeScript", "Python", "Rust"]
    }
  ]
}

If there is nothing useful to ask, return {"questions":[]}."#
        .to_string()
}

fn init_questionnaire_user_prompt(prose: &str) -> String {
    format!(
        "Build the init questionnaire for this project idea:\n\n<project-idea>\n{}\n</project-idea>\n",
        prose.trim()
    )
}

fn run_init_questionnaire(questions: Vec<InitQuestion>) -> Result<InitPreferences> {
    let mut terminal = ratatui::init();
    let result = run_init_questionnaire_loop(&mut terminal, questions);
    ratatui::restore();
    result
}

fn run_init_questionnaire_loop(
    terminal: &mut ratatui::DefaultTerminal,
    questions: Vec<InitQuestion>,
) -> Result<InitPreferences> {
    let mut app = InitQuestionnaire::new(questions);

    loop {
        terminal.draw(|frame| draw_init_questionnaire(frame, &app))?;

        if app.done {
            break;
        }

        if event::poll(Duration::from_millis(100)).context("failed to poll terminal events")?
            && let Event::Key(key) = event::read().context("failed to read terminal event")?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    bail!("init questionnaire cancelled");
                }
                KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.finish();
                }
                KeyCode::Up => app.select_previous(),
                KeyCode::Down => app.select_next(),
                KeyCode::Enter => app.accept_selected(),
                KeyCode::Esc => app.skip_current(),
                KeyCode::Backspace => app.backspace(),
                KeyCode::Char(ch) => app.push_char(ch),
                _ => {}
            }
        }
    }

    Ok(app.answers)
}

fn draw_init_questionnaire(frame: &mut Frame<'_>, app: &InitQuestionnaire) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Min(8),
            Constraint::Length(5),
        ])
        .split(area);

    let question = app.active_question();
    let header = Paragraph::new(Text::from(vec![
        Line::from(vec![
            Span::styled(
                "SpecForge init",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("  {}/{}", app.current + 1, app.questions.len())),
        ]),
        Line::from(question.prompt.as_str()),
        Line::from("Enter accepts, Esc skips, arrows move. Ctrl-D finishes, Ctrl-C cancels."),
    ]))
    .block(
        Block::default()
            .title(question.label.as_str())
            .borders(Borders::ALL),
    )
    .wrap(Wrap { trim: true });
    frame.render_widget(header, chunks[0]);

    let items = question
        .options
        .iter()
        .map(String::as_str)
        .chain(std::iter::once("Custom"))
        .enumerate()
        .map(|(idx, option)| {
            let selected = idx == app.selected && !app.editing_custom;
            let marker = if selected { ">" } else { " " };
            let style = if selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            ListItem::new(Line::from(vec![
                Span::styled(marker, style),
                Span::raw(" "),
                Span::styled(option, style),
            ]))
        })
        .collect::<Vec<_>>();
    let list = List::new(items).block(Block::default().title("Options").borders(Borders::ALL));
    frame.render_widget(list, chunks[1]);

    let custom = if app.editing_custom {
        format!("Custom: {}", app.custom_input)
    } else {
        "Choose Custom to type your own value.".to_string()
    };
    let footer = Paragraph::new(custom)
        .block(Block::default().title("Free input").borders(Borders::ALL))
        .wrap(Wrap { trim: false });
    frame.render_widget(footer, chunks[2]);
}

fn init_user_prompt(prose: &str, preferences: &InitPreferences) -> String {
    format!(
        "Create the initial SpecForge AsciiDoc spec from this project idea:\n\n<project-idea>\n{}\n</project-idea>\n\nUse these user-selected implementation preferences when they are provided:\n\n<init-preferences>\n{}\n</init-preferences>\n",
        prose.trim(),
        preferences.prompt_block()
    )
}

fn strip_json_fence(text: &str) -> &str {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let Some(end) = rest.rfind("```") else {
        return trimmed;
    };

    let inner = &rest[..end];
    inner
        .strip_prefix("json\n")
        .or_else(|| inner.strip_prefix("JSON\n"))
        .unwrap_or(inner)
        .trim()
}

fn starter_template(output: &Path) -> String {
    let project_id = output
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .map(slugify)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "my-project".to_string());

    format!(
        "= Project Specification\n:spec-version: 1\n:project-id: {project_id}\n\n== Project\n\nName:: Project\nLanguage:: Unknown\n\nInitial project specification.\n"
    )
}

fn slugify(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_init_checks_plan_and_requires_timeout() {
        let plan = parse_init_checks_plan(
            r#"{
                "checks": [
                    {
                        "command": ["cargo", "test", "--color", "never"],
                        "timeout_seconds": 120
                    },
                    {
                        "command": ["npm", "test"]
                    }
                ]
            }"#,
        )
        .expect("checks plan should parse");

        let checks = plan
            .checks
            .into_iter()
            .filter_map(normalize_init_check)
            .collect::<Vec<_>>();

        assert_eq!(
            checks,
            vec![ProjectCheckConfig {
                command: vec![
                    "cargo".to_string(),
                    "test".to_string(),
                    "--color".to_string(),
                    "never".to_string()
                ],
                timeout_seconds: 120,
            }]
        );
    }
}
