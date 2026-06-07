use std::{
    future::Future,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};

use anyhow::{Context, Result};
use ratatui::{
    Frame,
    crossterm::event::{self, Event, KeyCode, KeyEventKind},
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Wrap},
};
use specforge::agent::{DevelopmentAgentEvent, TaskChecklistItem, TaskStepStatus};

#[derive(Debug, Clone)]
pub enum ProgressEvent {
    Progress {
        current: usize,
        total: usize,
        label: String,
    },
    Label(String),
    Tasks(Vec<TaskChecklistItem>),
    Log(String),
    Finished {
        success: bool,
    },
}

#[derive(Clone)]
pub struct ProgressReporter {
    tx: Sender<ProgressEvent>,
}

impl ProgressReporter {
    pub fn progress(&self, current: usize, total: usize, label: impl Into<String>) {
        self.send(ProgressEvent::Progress {
            current,
            total,
            label: label.into(),
        });
    }

    pub fn tasks(&self, tasks: Vec<TaskChecklistItem>) {
        self.send(ProgressEvent::Tasks(tasks));
    }

    pub fn label(&self, label: impl Into<String>) {
        self.send(ProgressEvent::Label(label.into()));
    }

    pub fn log(&self, line: impl Into<String>) {
        self.send(ProgressEvent::Log(line.into()));
    }

    pub fn agent_event(&self, event: DevelopmentAgentEvent) {
        match event {
            DevelopmentAgentEvent::TaskCreated {
                task_dir,
                checklist,
                max_steps: _,
            } => {
                self.tasks(checklist);
                self.label("Execution task created");
                self.log(format!("Task: {}", task_dir.display()));
            }
            DevelopmentAgentEvent::TaskResumed {
                task_dir,
                checklist,
                max_steps: _,
            } => {
                self.tasks(checklist);
                self.label("Execution task resumed");
                self.log(format!("Task: {}", task_dir.display()));
            }
            DevelopmentAgentEvent::ChecklistUpdated(checklist) => {
                self.tasks(checklist);
            }
            DevelopmentAgentEvent::StepStarted { step, max_steps: _ } => {
                self.label(format!("Agent turn {step}"));
            }
            DevelopmentAgentEvent::AgentTurnCompleted { step, tool_calls } => {
                if tool_calls.is_empty() {
                    self.log(format!("Turn {step}: model returned a final answer"));
                } else {
                    self.log(format!("Turn {step}: {}", tool_calls.join(", ")));
                }
            }
            DevelopmentAgentEvent::ToolStarted { name } => {
                self.log(format!("Starting tool `{name}`"));
            }
            DevelopmentAgentEvent::ToolFinished { name, ok } => {
                let status = match ok {
                    Some(true) => "ok",
                    Some(false) => "failed",
                    None => "done",
                };
                self.log(format!("Tool `{name}` {status}"));
            }
            DevelopmentAgentEvent::Log(line) => {
                self.log(line);
            }
            DevelopmentAgentEvent::Finished {
                completed,
                checklist,
                final_answer,
            } => {
                let total = checklist.len().max(1);
                let current = if completed {
                    total
                } else {
                    checklist
                        .iter()
                        .filter(|item| item.status == TaskStepStatus::Completed)
                        .count()
                };
                self.progress(
                    current,
                    total,
                    if completed {
                        "Execution task completed"
                    } else {
                        "Execution task paused"
                    },
                );
                self.tasks(checklist);
                if !final_answer.trim().is_empty() {
                    self.log(final_answer.trim());
                }
            }
        }
    }

    fn send(&self, event: ProgressEvent) {
        let _ = self.tx.send(event);
    }
}

pub async fn run_with_tui<T, Fut, F>(title: impl Into<String>, operation: F) -> Result<T>
where
    F: FnOnce(ProgressReporter) -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let (tx, rx) = mpsc::channel();
    let ui_title = title.into();
    let ui_handle = thread::spawn(move || run_ui(ui_title, rx));
    let reporter = ProgressReporter { tx: tx.clone() };

    let result = operation(reporter).await;
    let _ = tx.send(ProgressEvent::Finished {
        success: result.is_ok(),
    });
    drop(tx);

    let ui_result = ui_handle
        .join()
        .map_err(|_| anyhow::anyhow!("TUI thread panicked"))?;
    if result.is_ok() {
        ui_result?;
    }

    result
}

fn run_ui(title: String, rx: Receiver<ProgressEvent>) -> Result<()> {
    let mut terminal = ratatui::init();
    let result = run_ui_loop(&mut terminal, title, rx);
    ratatui::restore();
    result
}

fn run_ui_loop(
    terminal: &mut ratatui::DefaultTerminal,
    title: String,
    rx: Receiver<ProgressEvent>,
) -> Result<()> {
    let mut app = TuiApp::new(title);

    loop {
        while let Ok(event) = rx.try_recv() {
            app.apply(event);
        }

        terminal.draw(|frame| draw(frame, &mut app))?;

        if app.done {
            break;
        }

        if event::poll(Duration::from_millis(80)).context("failed to poll terminal events")?
            && let Event::Key(key) = event::read().context("failed to read terminal event")?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Up => app.scroll_up(1),
                KeyCode::Down => app.scroll_down(1),
                KeyCode::PageUp => app.scroll_up(10),
                KeyCode::PageDown => app.scroll_down(10),
                KeyCode::Home => app.scroll_home(),
                KeyCode::End => app.scroll_end(),
                _ => {}
            }
        }
    }

    Ok(())
}

struct TuiApp {
    title: String,
    progress_current: usize,
    progress_total: usize,
    progress_label: String,
    tasks: Vec<TaskChecklistItem>,
    logs: Vec<String>,
    log_scroll: usize,
    follow_log: bool,
    done: bool,
    success: bool,
}

impl TuiApp {
    fn new(title: String) -> Self {
        Self {
            title,
            progress_current: 0,
            progress_total: 1,
            progress_label: "Starting".to_string(),
            tasks: Vec::new(),
            logs: vec!["Starting".to_string()],
            log_scroll: 0,
            follow_log: true,
            done: false,
            success: true,
        }
    }

    fn apply(&mut self, event: ProgressEvent) {
        match event {
            ProgressEvent::Progress {
                current,
                total,
                label,
            } => {
                self.progress_current = current.min(total);
                self.progress_total = total.max(1);
                self.progress_label = label;
            }
            ProgressEvent::Label(label) => {
                self.progress_label = label;
            }
            ProgressEvent::Tasks(tasks) => {
                self.tasks = tasks;
                self.update_task_progress();
            }
            ProgressEvent::Log(line) => {
                self.logs.extend(line.lines().map(str::to_string));
            }
            ProgressEvent::Finished { success } => {
                self.success = success;
                self.done = true;
                self.progress_label = if success {
                    "Finished".to_string()
                } else {
                    "Failed".to_string()
                };
            }
        }
    }

    fn scroll_up(&mut self, amount: usize) {
        self.follow_log = false;
        self.log_scroll = self.log_scroll.saturating_sub(amount);
    }

    fn scroll_down(&mut self, amount: usize) {
        self.log_scroll = self.log_scroll.saturating_add(amount);
    }

    fn scroll_home(&mut self) {
        self.follow_log = false;
        self.log_scroll = 0;
    }

    fn scroll_end(&mut self) {
        self.follow_log = true;
    }

    fn progress_ratio(&self) -> f64 {
        if self.progress_total == 0 {
            return 0.0;
        }

        (self.progress_current as f64 / self.progress_total as f64).clamp(0.0, 1.0)
    }

    fn update_task_progress(&mut self) {
        let total = self.tasks.len().max(1);
        let current = self
            .tasks
            .iter()
            .filter(|item| item.status == TaskStepStatus::Completed)
            .count();

        self.progress_current = current.min(total);
        self.progress_total = total;
    }
}

fn draw(frame: &mut Frame<'_>, app: &mut TuiApp) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(8),
            Constraint::Min(8),
        ])
        .split(area);

    let gauge_style = if app.success {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::Red)
    };
    let progress = Gauge::default()
        .block(
            Block::default()
                .title(app.title.as_str())
                .borders(Borders::ALL),
        )
        .gauge_style(gauge_style)
        .ratio(app.progress_ratio())
        .label(app.progress_label.as_str());
    frame.render_widget(progress, chunks[0]);

    let tasks = if app.tasks.is_empty() {
        vec![ListItem::new(Line::from("No task state yet"))]
    } else {
        app.tasks
            .iter()
            .map(|item| {
                let (mark, style) = match item.status {
                    TaskStepStatus::Completed => ("[x]", Style::default().fg(Color::Green)),
                    TaskStepStatus::Pending => ("[ ]", Style::default().fg(Color::DarkGray)),
                };
                ListItem::new(Line::from(vec![
                    Span::styled(mark, style),
                    Span::raw(" "),
                    Span::raw(item.label.clone()),
                ]))
            })
            .collect()
    };
    let task_list = List::new(tasks)
        .block(Block::default().title("Tasks").borders(Borders::ALL))
        .highlight_style(Style::default().add_modifier(Modifier::BOLD));
    frame.render_widget(task_list, chunks[1]);

    let log_height = chunks[2].height.saturating_sub(2) as usize;
    let max_scroll = app.logs.len().saturating_sub(log_height);
    if app.follow_log {
        app.log_scroll = max_scroll;
    } else {
        app.log_scroll = app.log_scroll.min(max_scroll);
        if app.log_scroll == max_scroll {
            app.follow_log = true;
        }
    }

    let log_lines = app
        .logs
        .iter()
        .map(|line| Line::from(line.clone()))
        .collect::<Vec<_>>();
    let log = Paragraph::new(Text::from(log_lines))
        .block(Block::default().title("Log").borders(Borders::ALL))
        .scroll((app.log_scroll.min(u16::MAX as usize) as u16, 0))
        .wrap(Wrap { trim: false });
    frame.render_widget(log, chunks[2]);
}
