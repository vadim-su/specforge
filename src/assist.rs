use std::{
    io::{self, IsTerminal},
    path::PathBuf,
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
    context::ContextBundle,
    llm::{LlmClient, LlmPrompt},
    prompts,
    provider::Provider,
    spec::{Severity, parse_spec_file, print_diagnostics, validate_model},
};

#[derive(Debug)]
pub struct AssistExpandOptions {
    pub spec: PathBuf,
    pub focus: Option<String>,
    pub provider: Provider,
    pub model: Option<String>,
    pub no_tui: bool,
}

pub async fn expand_spec(options: AssistExpandOptions) -> Result<String> {
    let parsed = parse_spec_file(&options.spec)?;
    let diagnostics = validate_model(&parsed.model);
    print_diagnostics(&diagnostics);

    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        bail!("spec validation failed; assist did not run");
    }

    let root = std::env::current_dir().context("failed to read current directory")?;
    let project_context = ContextBundle::collect(&root, &options.spec)?;
    let user_prompt = expand_user_prompt(&options, &parsed.source, &project_context);
    let client = LlmClient::new(options.provider, options.model.clone());
    let questions = assist_questionnaire_plan(&client, user_prompt).await?;

    if questions.is_empty() {
        bail!("assist did not find useful expansion questions");
    }

    if options.no_tui || !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Ok(render_questions(&questions));
    }

    let answers = run_assist_questionnaire(questions)?;
    if answers.is_empty() {
        bail!("assist needs at least one answer to produce conclusions");
    }

    client
        .complete(LlmPrompt {
            system: assist_summary_system_prompt(),
            user: assist_summary_user_prompt(&options, &parsed.source, &project_context, &answers),
            temperature: Some(0.2),
        })
        .await
}

#[derive(Debug, Clone, Deserialize)]
struct AssistQuestionnairePlan {
    #[serde(default)]
    questions: Vec<AssistQuestion>,
}

#[derive(Debug, Clone, Deserialize)]
struct AssistQuestion {
    #[serde(default)]
    label: String,
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    options: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct AssistAnswers {
    items: Vec<AssistAnswer>,
}

impl AssistAnswers {
    fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn prompt_block(&self) -> String {
        self.items
            .iter()
            .map(|item| {
                format!(
                    "Question: {}\nAnswer: {}",
                    item.question.prompt, item.answer
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

#[derive(Debug, Clone)]
struct AssistAnswer {
    question: AssistQuestion,
    answer: String,
}

#[derive(Debug)]
struct AssistQuestionnaire {
    questions: Vec<AssistQuestion>,
    answers: AssistAnswers,
    current: usize,
    selected: usize,
    input: String,
    editing_custom: bool,
    done: bool,
}

impl AssistQuestionnaire {
    fn new(questions: Vec<AssistQuestion>) -> Self {
        Self {
            questions,
            answers: AssistAnswers::default(),
            current: 0,
            selected: 0,
            input: String::new(),
            editing_custom: false,
            done: false,
        }
    }

    fn active_question(&self) -> &AssistQuestion {
        &self.questions[self.current]
    }

    fn select_next(&mut self) {
        if self.editing_custom || self.active_question().options.is_empty() {
            return;
        }

        let option_count = self.active_question().options.len() + 1;
        self.selected = (self.selected + 1).min(option_count.saturating_sub(1));
    }

    fn select_previous(&mut self) {
        if self.editing_custom || self.active_question().options.is_empty() {
            return;
        }

        self.selected = self.selected.saturating_sub(1);
    }

    fn accept_current(&mut self) {
        let question = self.active_question();
        if !self.editing_custom && !question.options.is_empty() {
            if self.selected == question.options.len() {
                self.editing_custom = true;
                return;
            }

            self.accept_answer(question.options[self.selected].clone());
            return;
        }

        let answer = self.input.trim().to_string();
        if !answer.is_empty() {
            self.accept_answer(answer);
        } else {
            self.advance();
        }
    }

    fn skip_current(&mut self) {
        if self.editing_custom {
            self.editing_custom = false;
            self.input.clear();
            return;
        }

        self.advance();
    }

    fn finish(&mut self) {
        self.done = true;
    }

    fn push_char(&mut self, ch: char) {
        if self.editing_custom || self.active_question().options.is_empty() {
            self.input.push(ch);
        }
    }

    fn backspace(&mut self) {
        if self.editing_custom || self.active_question().options.is_empty() {
            self.input.pop();
        }
    }

    fn accept_answer(&mut self, answer: String) {
        let question = self.active_question().clone();
        self.answers.items.push(AssistAnswer { question, answer });
        self.advance();
    }

    fn advance(&mut self) {
        self.selected = 0;
        self.input.clear();
        self.editing_custom = false;
        if self.current + 1 >= self.questions.len() {
            self.done = true;
        } else {
            self.current += 1;
        }
    }
}

async fn assist_questionnaire_plan(
    client: &LlmClient,
    user_prompt: String,
) -> Result<Vec<AssistQuestion>> {
    let response = client
        .complete(LlmPrompt {
            system: prompts::ASSIST_EXPAND_SYSTEM.to_string(),
            user: user_prompt,
            temperature: Some(0.2),
        })
        .await?;
    let plan: AssistQuestionnairePlan = serde_json::from_str(strip_json_fence(&response))
        .context("failed to parse assist questionnaire JSON from LLM")?;

    Ok(plan
        .questions
        .into_iter()
        .filter_map(normalize_assist_question)
        .take(12)
        .collect())
}

fn normalize_assist_question(mut question: AssistQuestion) -> Option<AssistQuestion> {
    question.label = question.label.trim().to_string();
    question.prompt = question.prompt.trim().to_string();
    question.options = question
        .options
        .into_iter()
        .map(|option| option.trim().to_string())
        .filter(|option| !option.is_empty() && !option.eq_ignore_ascii_case("custom"))
        .take(5)
        .collect();

    if question.label.is_empty() || question.prompt.is_empty() {
        return None;
    }

    Some(question)
}

fn render_questions(questions: &[AssistQuestion]) -> String {
    let items = questions
        .iter()
        .enumerate()
        .map(|(idx, question)| {
            let options = if question.options.is_empty() {
                String::new()
            } else {
                format!("\n   Options: {}", question.options.join(", "))
            };
            format!(
                "{}. [{}] {}{}",
                idx + 1,
                question.label,
                question.prompt,
                options
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "Interactive TUI is unavailable or disabled. Generated questions:\n\n{items}\n\nRun without --no-tui in an interactive terminal to answer them and get conclusions."
    )
}

fn expand_user_prompt(
    options: &AssistExpandOptions,
    spec_source: &str,
    context: &ContextBundle,
) -> String {
    let focus = options
        .focus
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("No explicit focus was provided.");

    format!(
        "Review this SpecForge spec and the project context. Build the interactive expansion questionnaire.\n\nFocus: {focus}\n\n<spec path=\"{}\">\n{}\n</spec>\n\n<technology-profiles>\n{}\n</technology-profiles>\n\n<context-integrations>\n{}\n</context-integrations>\n\n<project-files truncated=\"{}\">\n{}\n</project-files>\n\n<context-files>\n{}\n</context-files>\n",
        options.spec.display(),
        spec_source.trim(),
        context.render_profiles_prompt(),
        context.render_integrations_prompt(),
        context.files_truncated,
        context.render_project_files_prompt(),
        context.render_file_snippets_prompt()
    )
}

fn run_assist_questionnaire(questions: Vec<AssistQuestion>) -> Result<AssistAnswers> {
    let mut terminal = ratatui::init();
    let result = run_assist_questionnaire_loop(&mut terminal, questions);
    ratatui::restore();
    result
}

fn run_assist_questionnaire_loop(
    terminal: &mut ratatui::DefaultTerminal,
    questions: Vec<AssistQuestion>,
) -> Result<AssistAnswers> {
    let mut app = AssistQuestionnaire::new(questions);

    loop {
        terminal.draw(|frame| draw_assist_questionnaire(frame, &app))?;

        if app.done {
            break;
        }

        if event::poll(Duration::from_millis(100)).context("failed to poll terminal events")?
            && let Event::Key(key) = event::read().context("failed to read terminal event")?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    bail!("assist questionnaire cancelled");
                }
                KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.finish();
                }
                KeyCode::Up => app.select_previous(),
                KeyCode::Down => app.select_next(),
                KeyCode::Enter => app.accept_current(),
                KeyCode::Esc => app.skip_current(),
                KeyCode::Backspace => app.backspace(),
                KeyCode::Char(ch) => app.push_char(ch),
                _ => {}
            }
        }
    }

    Ok(app.answers)
}

fn draw_assist_questionnaire(frame: &mut Frame<'_>, app: &AssistQuestionnaire) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6),
            Constraint::Min(7),
            Constraint::Length(7),
            Constraint::Min(6),
        ])
        .split(area);

    let question = app.active_question();
    let header = Paragraph::new(Text::from(vec![
        Line::from(vec![
            Span::styled(
                "SpecForge assist",
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

    let option_items = if question.options.is_empty() {
        vec![ListItem::new(Line::from("Free answer"))]
    } else {
        question
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
            .collect()
    };
    let options =
        List::new(option_items).block(Block::default().title("Options").borders(Borders::ALL));
    frame.render_widget(options, chunks[1]);

    let input_title = if app.editing_custom || question.options.is_empty() {
        "Answer"
    } else {
        "Free input"
    };
    let input_text = if app.editing_custom || question.options.is_empty() {
        app.input.clone()
    } else {
        "Choose Custom to type your own value.".to_string()
    };
    let answer = Paragraph::new(input_text)
        .block(Block::default().title(input_title).borders(Borders::ALL))
        .wrap(Wrap { trim: false });
    frame.render_widget(answer, chunks[2]);

    let answered = if app.answers.items.is_empty() {
        vec![ListItem::new(Line::from("No answers yet"))]
    } else {
        app.answers
            .items
            .iter()
            .map(|item| {
                ListItem::new(Line::from(vec![
                    Span::styled("[x]", Style::default().fg(Color::Green)),
                    Span::raw(" "),
                    Span::raw(item.question.label.clone()),
                    Span::raw(": "),
                    Span::raw(item.answer.clone()),
                ]))
            })
            .collect()
    };
    let list = List::new(answered).block(Block::default().title("Answered").borders(Borders::ALL));
    frame.render_widget(list, chunks[3]);
}

fn assist_summary_system_prompt() -> String {
    r#"You are a senior product and engineering spec reviewer for SpecForge projects.

Return only concise Markdown. Do not rewrite the full spec.

Use the user's answers to turn the questionnaire into actionable conclusions:

- Summarize the product/spec decisions that are now clear.
- List concrete spec additions or changes the user should make.
- Call out still-open questions only when important.
- Mention relevant file paths when the conclusion is grounded in project context.
- Use the same language as the spec. If the spec language is mixed, use the dominant language in user-authored prose.
- Prefer specific acceptance criteria, entity fields, command behavior, constraints, and decisions over broad advice."#
        .to_string()
}

fn assist_summary_user_prompt(
    options: &AssistExpandOptions,
    spec_source: &str,
    context: &ContextBundle,
    answers: &AssistAnswers,
) -> String {
    let focus = options
        .focus
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("No explicit focus was provided.");

    format!(
        "Create conclusions for expanding this SpecForge spec from the user's questionnaire answers.\n\nFocus: {focus}\n\n<spec path=\"{}\">\n{}\n</spec>\n\n<technology-profiles>\n{}\n</technology-profiles>\n\n<context-integrations>\n{}\n</context-integrations>\n\n<project-files truncated=\"{}\">\n{}\n</project-files>\n\n<context-files>\n{}\n</context-files>\n\n<answers>\n{}\n</answers>\n",
        options.spec.display(),
        spec_source.trim(),
        context.render_profiles_prompt(),
        context.render_integrations_prompt(),
        context.files_truncated,
        context.render_project_files_prompt(),
        context.render_file_snippets_prompt(),
        answers.prompt_block()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_assist_question_options() {
        let question = normalize_assist_question(AssistQuestion {
            label: " Priority ".to_string(),
            prompt: " Choose priority behavior? ".to_string(),
            options: vec![
                " High first ".to_string(),
                "Custom".to_string(),
                String::new(),
                " Low first ".to_string(),
            ],
        })
        .expect("question should normalize");

        assert_eq!(question.label, "Priority");
        assert_eq!(question.prompt, "Choose priority behavior?");
        assert_eq!(question.options, vec!["High first", "Low first"]);
    }
}
