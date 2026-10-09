//! A full terminal interface for APEX, built on ratatui.
//!
//! `apex tui` connects to the running runtime and renders a live view: tasks on
//! the left, the selected task's event stream in the centre, and its plan and
//! subtasks on the right. It is a frontend only — every action is translated
//! into a protocol message and every piece of state comes from the runtime.

use std::io::stdout;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use apex_core::config::Config;
use apex_protocol::{Event, ExecutionMode, Task, TaskStatus, TeamSpec};
use crossterm::event::{self, Event as CrosstermEvent, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Terminal;

use crate::daemon;
use crate::render;

/// How often to poll for new events and task state.
const POLL_INTERVAL: Duration = Duration::from_millis(350);

/// Run the TUI until the user quits.
pub async fn run(config: Config) -> Result<()> {
    let mut client = daemon::connect_or_start(&config).await?;
    let mut state = TuiState::new();

    // Initial data.
    state.tasks = fetch_tasks(&mut client).await?;

    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    let result = event_loop(&mut terminal, &mut client, &mut state).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

async fn fetch_tasks(client: &mut apex_runtime::client::Client) -> Result<Vec<Task>> {
    match client
        .request(apex_protocol::Request::ListTasks { limit: Some(100) })
        .await?
    {
        Response::TaskList { tasks } => Ok(tasks.into_iter().collect()),
        other => bail!("unexpected response listing tasks: {other:?}"),
    }
}

use apex_protocol::Response;

/// Application state for the interface.
pub struct TuiState {
    tasks: Vec<Task>,
    selected: usize,
    events: Vec<Event>,
    /// Per-task event streams, keyed by task id.
    events_by_task: std::collections::HashMap<String, Vec<Event>>,
    subtasks: Vec<apex_protocol::Subtask>,
    /// Plan waves for the selected task.
    waves: Vec<usize>,
    /// Currently focused pane.
    focus: Focus,
    /// Scroll offset within the event pane.
    scroll: u16,
    /// Status message shown in the footer.
    status: String,
    /// Whether a prompt is awaiting input.
    prompt: Option<String>,
    input: String,
    /// The most recent diff, shown in its own pane when present.
    diff: Option<String>,
}

impl Default for TuiState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Tasks,
    Events,
    Plan,
}

impl TuiState {
    /// Create an empty state.
    pub fn new() -> TuiState {
        TuiState {
            tasks: Vec::new(),
            selected: 0,
            events: Vec::new(),
            events_by_task: std::collections::HashMap::new(),
            subtasks: Vec::new(),
            waves: Vec::new(),
            focus: Focus::Tasks,
            scroll: 0,
            status: "ready".to_string(),
            prompt: None,
            input: String::new(),
            diff: None,
        }
    }

    /// The currently selected task, if any.
    pub fn current(&self) -> Option<&Task> {
        self.tasks.get(self.selected)
    }

    /// Number of tasks currently known.
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    /// Index of the selected task.
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// The loaded diff, if one has been fetched.
    pub fn diff(&self) -> Option<&str> {
        self.diff.as_deref()
    }

    /// Store a diff for the diff pane.
    pub fn set_diff(&mut self, diff: Option<String>) {
        self.diff = diff;
    }

    /// Add a task to the list.
    pub fn push_task(&mut self, task: Task) {
        self.tasks.push(task);
    }

    /// Set the selected index directly.
    pub fn set_selected(&mut self, index: usize) {
        self.selected = index;
    }

    pub fn clamp_selection(&mut self) {
        if self.tasks.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.tasks.len() {
            self.selected = self.tasks.len() - 1;
        }
    }

    /// Start a prompt for the given action.
    fn begin_prompt(&mut self, label: String) {
        self.prompt = Some(label);
        self.input.clear();
    }
}

/// The main loop: poll for input and new data, then draw.
async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    client: &mut apex_runtime::client::Client,
    state: &mut TuiState,
) -> Result<()> {
    let mut last_tick = Instant::now();

    loop {
        // Live events from the runtime.
        while let Ok(event) = client.events.try_recv() {
            let task_id = event.task_id.clone();
            state
                .events_by_task
                .entry(task_id.clone())
                .or_default()
                .push(event.clone());
            if state.current().map(|t| t.id == task_id).unwrap_or(false) {
                state.events.push(event);
                state.scroll = u16::MAX / 2;
            }
        }

        // Periodic refresh of task and subtask state.
        if last_tick.elapsed() >= POLL_INTERVAL {
            last_tick = Instant::now();
            if state.prompt.is_none() {
                refresh(client, state).await;
            }
        }

        terminal.draw(|frame| draw(frame, state))?;

        let has_input = event::poll(Duration::from_millis(50))?;
        if !has_input {
            continue;
        }

        match event::read()? {
            CrosstermEvent::Key(key) => {
                if key.kind != crossterm::event::KeyEventKind::Press {
                    continue;
                }
                if handle_key(client, state, key).await? {
                    return Ok(());
                }
            }
            CrosstermEvent::Resize(_, _) => {}
            _ => {}
        }
    }
}

/// Fetch fresh task, subtask and plan state.
async fn refresh(client: &mut apex_runtime::client::Client, state: &mut TuiState) {
    match fetch_tasks(client).await {
        Ok(tasks) => {
            if tasks.len() != state.tasks.len() {
                state.clamp_selection();
            }
            state.tasks = tasks;
        }
        Err(error) => state.status = format!("could not refresh tasks: {error}"),
    }

    if let Some(task) = state.current().cloned() {
        state.events = state
            .events_by_task
            .get(&task.id)
            .cloned()
            .unwrap_or_default();
        fetch_subtasks_and_plan(client, state, &task.id).await;
    }
}

async fn fetch_subtasks_and_plan(
    client: &mut apex_runtime::client::Client,
    state: &mut TuiState,
    task_id: &str,
) {
    match client
        .request(apex_protocol::Request::ListSubtasks {
            task_id: task_id.to_string(),
        })
        .await
    {
        Ok(Response::SubtaskList { subtasks, .. }) => {
            state.subtasks = subtasks.into_iter().collect()
        }
        Ok(_) => {}
        Err(error) => state.status = format!("could not load subtasks: {error}"),
    }

    match client
        .request(apex_protocol::Request::ShowPlan {
            task_id: task_id.to_string(),
        })
        .await
    {
        Ok(Response::Plan { waves, .. }) => state.waves = *waves,
        Ok(_) => {}
        Err(_) => state.waves.clear(),
    }
}

/// Handle a key press. Returns true when the interface should exit.
async fn handle_key(
    client: &mut apex_runtime::client::Client,
    state: &mut TuiState,
    key: KeyEvent,
) -> Result<bool> {
    // While a prompt is open, only the prompt consumes keys.
    if state.prompt.is_some() {
        return handle_prompt_key(client, state, key).await;
    }

    match (key.code, key.modifiers) {
        (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(true),
        (KeyCode::Char('?'), _) => state.status = help_text(),
        (KeyCode::Char('r'), _) => {
            state.status = "refreshing".to_string();
            refresh(client, state).await;
        }
        (KeyCode::Tab, _) => state.focus = next_focus(state.focus),
        (KeyCode::Char('n'), _) => {
            state.begin_prompt("New task: describe the objective".into());
        }
        (KeyCode::Char('a'), _) => {
            state.begin_prompt("New task with agents: objective [--agents a,b]".into());
        }
        (KeyCode::Char('w'), _) => {
            state.begin_prompt("Run workflow: objective --workflow file.toml".into());
        }
        (KeyCode::Char('c'), _) => {
            if let Some(task) = state.current().cloned() {
                cancel_task(client, &task.id).await;
            }
        }
        (KeyCode::Char('d'), _) => {
            if let Some(task) = state.current().cloned() {
                show_diff(client, state, &task.id).await;
            }
        }
        (KeyCode::Char('v'), _) => {
            if let Some(task) = state.current().cloned() {
                verify(client, &task.id).await;
            }
        }
        (KeyCode::Up, _) => match state.focus {
            Focus::Tasks => {
                state.selected = state.selected.saturating_sub(1);
                select_current(client, state).await;
            }
            Focus::Events => state.scroll = state.scroll.saturating_sub(1),
            Focus::Plan => state.scroll = state.scroll.saturating_sub(1),
        },
        (KeyCode::Down, _) => match state.focus {
            Focus::Tasks => {
                if !state.tasks.is_empty() && state.selected + 1 < state.tasks.len() {
                    state.selected += 1;
                }
                select_current(client, state).await;
            }
            Focus::Events => state.scroll = state.scroll.saturating_add(1),
            Focus::Plan => state.scroll = state.scroll.saturating_add(1),
        },
        _ => {}
    }
    Ok(false)
}

fn next_focus(current: Focus) -> Focus {
    match current {
        Focus::Tasks => Focus::Events,
        Focus::Events => Focus::Plan,
        Focus::Plan => Focus::Tasks,
    }
}

fn help_text() -> String {
    "q quit · r refresh · Tab switch pane · ↑↓ navigate · n new task · a agent team · \
     w workflow · c cancel · d diff · v verify · ? hide this"
        .into()
}

/// Handle keys while a prompt is open: Enter submits, Escape cancels.
async fn handle_prompt_key(
    client: &mut apex_runtime::client::Client,
    state: &mut TuiState,
    key: KeyEvent,
) -> Result<bool> {
    match key.code {
        KeyCode::Esc => {
            state.prompt = None;
            state.input.clear();
        }
        KeyCode::Enter => {
            state.prompt.take();
            let input = std::mem::take(&mut state.input);
            submit_prompt(client, state, &input).await;
        }
        KeyCode::Backspace => {
            state.input.pop();
        }
        KeyCode::Char(c) => state.input.push(c),
        _ => {}
    }
    Ok(false)
}

/// Parse a prompt into a protocol request and dispatch it.
async fn submit_prompt(
    client: &mut apex_runtime::client::Client,
    state: &mut TuiState,
    input: &str,
) {
    let input = input.trim();
    if input.is_empty() {
        state.status = "cancelled: empty input".to_string();
        return;
    }

    let mut parts = input.split_whitespace();
    let mut agents: Option<Vec<String>> = None;
    let mut workflow: Option<String> = None;

    // Pull recognised flags out of the input; the rest is the objective.
    let mut words: Vec<&str> = Vec::new();
    while let Some(word) = parts.next() {
        match word {
            "--agents" => {
                if let Some(list) = parts.next() {
                    agents = Some(list.split(',').map(|s| s.trim().to_string()).collect());
                }
            }
            "--workflow" => {
                workflow = parts.next().map(|s| s.to_string());
            }
            other => words.push(other),
        }
    }
    let objective = words.join(" ");

    if let Some(agents) = agents {
        state.status = format!("starting team of {}", agents.len());
        let mut spec = TeamSpec::new(agents.iter().cloned());
        spec.strategy = apex_protocol::TeamStrategy::Sequential;
        if let Err(error) = client
            .request(apex_protocol::Request::CreateTask {
                objective: objective.clone(),
                project_root: std::env::current_dir()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                model: None,
                agent_id: None,
                mode: ExecutionMode::ManualMulti,
                team: Some(spec),
                plan: None,
                workflow: None,
            })
            .await
        {
            state.status = format!("could not start team: {error}");
        }
    } else if let Some(workflow) = workflow {
        match apex_orchestrator::WorkflowDefinition::load(std::path::Path::new(&workflow)) {
            Ok(definition) => {
                let result = client
                    .request(apex_protocol::Request::CreateTask {
                        objective: objective.clone(),
                        project_root: std::env::current_dir()
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        model: None,
                        agent_id: None,
                        mode: ExecutionMode::Workflow,
                        team: None,
                        plan: None,
                        workflow: Some(definition),
                    })
                    .await;
                if let Err(error) = result {
                    state.status = format!("could not start workflow: {error}");
                }
            }
            Err(error) => state.status = format!("could not read workflow: {error}"),
        }
    } else {
        state.status = "starting task".to_string();
        if let Err(error) = client
            .request(apex_protocol::Request::CreateTask {
                objective: objective.clone(),
                project_root: std::env::current_dir()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                model: None,
                agent_id: None,
                mode: ExecutionMode::Single,
                team: None,
                plan: None,
                workflow: None,
            })
            .await
        {
            state.status = format!("could not start task: {error}");
        }
    }
}

/// Load the selected task's events into the pane.
async fn select_current(client: &mut apex_runtime::client::Client, state: &mut TuiState) {
    let Some(task) = state.current().cloned() else {
        state.events.clear();
        state.subtasks.clear();
        state.waves.clear();
        return;
    };
    state.events = state
        .events_by_task
        .get(&task.id)
        .cloned()
        .unwrap_or_default();
    fetch_subtasks_and_plan(client, state, &task.id).await;
    state.status = format!("selected {}", task.id);
}

async fn cancel_task(client: &mut apex_runtime::client::Client, task_id: &str) {
    match client
        .request(apex_protocol::Request::CancelTask {
            task_id: task_id.to_string(),
        })
        .await
    {
        Ok(_) => {}
        Err(error) => eprintln!("could not cancel task: {error}"),
    }
}

/// Load the selected task's diff and store it for the diff pane.
async fn show_diff(client: &mut apex_runtime::client::Client, state: &mut TuiState, task_id: &str) {
    match client
        .request(apex_protocol::Request::GetDiff {
            task_id: task_id.to_string(),
        })
        .await
    {
        Ok(Response::Diff { diff }) => {
            state.diff = Some(*diff);
            state.status = "diff loaded".to_string();
        }
        Ok(_) => {}
        Err(error) => eprintln!("could not load diff: {error}"),
    }
}

/// Report a verification result without disturbing the interface state.
async fn verify(client: &mut apex_runtime::client::Client, task_id: &str) {
    match client
        .request(apex_protocol::Request::Verify {
            task_id: task_id.to_string(),
        })
        .await
    {
        Ok(Response::Verification { passed, checks }) => {
            let summary = checks
                .iter()
                .map(|c| format!("{}: {}", c.name, if c.passed { "PASS" } else { "FAIL" }))
                .collect::<Vec<_>>()
                .join(" | ");
            eprintln!(
                "verification {} — {summary}",
                if passed { "passed" } else { "failed" }
            );
        }
        Ok(_) => {}
        Err(error) => eprintln!("could not verify: {error}"),
    }
}

// ---------------------------------------------------------------- rendering

fn draw(frame: &mut ratatui::Frame, state: &TuiState) {
    let area = frame.area();

    let has_diff = state.diff.is_some();
    let rows = if has_diff {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Percentage(60),
                Constraint::Percentage(40),
                Constraint::Length(12),
                Constraint::Length(1),
            ])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(4),
                Constraint::Length(12),
                Constraint::Length(1),
            ])
            .split(area)
    };

    frame.render_widget(title_block(), rows[0]);

    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(24),
            Constraint::Percentage(46),
            Constraint::Percentage(30),
        ])
        .split(rows[1]);

    render_tasks(frame, panes[0], state);
    render_events(frame, panes[1], state);
    render_plan(frame, panes[2], state);

    if has_diff {
        render_diff(frame, rows[2], state);
        render_footer(frame, rows[3], state);
        render_status_line(frame, rows[4], state);
    } else {
        render_footer(frame, rows[2], state);
        render_status_line(frame, rows[3], state);
    }

    // Overlay a prompt if one is open.
    if state.prompt.is_some() {
        render_prompt(frame, area, state);
    }
}

fn title_block() -> Paragraph<'static> {
    Paragraph::new(Line::from(vec![
        Span::styled(
            " APEX ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "autonomous software engineering — live runtime view",
            Style::default().fg(Color::DarkGray),
        ),
    ]))
    .block(Block::default().borders(Borders::BOTTOM))
}

fn render_tasks(frame: &mut ratatui::Frame, area: Rect, state: &TuiState) {
    let items: Vec<ListItem> = state
        .tasks
        .iter()
        .map(|task| {
            let status = render::status_label(task.status);
            let objective = render::truncate(&task.objective, 34);
            ListItem::new(Line::from(vec![
                Span::styled(format!("{} ", status), Style::default()),
                Span::styled(objective, Style::default()),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(Span::styled(
            format!(" Tasks ({}) ", state.tasks.len()),
            Style::default().fg(if state.focus == Focus::Tasks {
                Color::Cyan
            } else {
                Color::Gray
            }),
        )))
        .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White));

    let mut list_state = ListState::default();
    if !state.tasks.is_empty() {
        list_state.select(Some(state.selected));
    }
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn render_events(frame: &mut ratatui::Frame, area: Rect, state: &TuiState) {
    let title = if let Some(task) = state.current() {
        format!(" Events — {} ", render::short_id(&task.id))
    } else {
        " Events ".to_string()
    };

    let mut lines: Vec<Line> = Vec::new();
    for event in &state.events {
        lines.extend(render::event_lines(event));
    }

    let paragraph = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(Span::styled(
            title,
            Style::default().fg(if state.focus == Focus::Events {
                Color::Cyan
            } else {
                Color::Gray
            }),
        )))
        .wrap(Wrap { trim: false })
        .scroll((state.scroll, 0));

    frame.render_widget(paragraph, area);
}

fn render_plan(frame: &mut ratatui::Frame, area: Rect, state: &TuiState) {
    let mut lines: Vec<Line> = Vec::new();

    if state.subtasks.is_empty() && state.waves.is_empty() {
        lines.push(Line::from(Span::styled(
            "(no multi-step plan — single-agent task)",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for (index, subtask) in state.subtasks.iter().enumerate() {
            let wave = state.waves.get(index).copied().unwrap_or(0);
            lines.push(Line::from(vec![
                Span::styled(format!("w{} ", wave), Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{:<16} {:<10}", subtask.agent_id, subtask.status.as_str()),
                    Style::default(),
                ),
            ]));
        }
    }

    if let Some(task) = state.current() {
        if let Some(error) = &task.error {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                render::truncate(error, 40),
                Style::default().fg(Color::Red),
            )));
        } else if let Some(summary) = &task.summary {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                render::truncate(summary, 40),
                Style::default().fg(Color::Green),
            )));
        }
    }

    let paragraph = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(Span::styled(
            " Plan & Subtasks ",
            Style::default().fg(if state.focus == Focus::Plan {
                Color::Cyan
            } else {
                Color::Gray
            }),
        )))
        .wrap(Wrap { trim: true })
        .scroll((state.scroll, 0));

    frame.render_widget(paragraph, area);
}

/// Render the most recent diff, when one has been loaded.
fn render_diff(frame: &mut ratatui::Frame, area: Rect, state: &TuiState) {
    let diff = state.diff.clone().unwrap_or_default();
    let lines: Vec<Line> = diff
        .lines()
        .map(|line| {
            let style = if line.starts_with('+') {
                Style::default().fg(Color::Green)
            } else if line.starts_with('-') {
                Style::default().fg(Color::Red)
            } else if line.starts_with("@@") {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            Line::from(Span::styled(line.to_string(), style))
        })
        .collect();

    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(" Diff "))
            .wrap(Wrap { trim: false })
            .scroll((0, 0)),
        area,
    );
}

fn render_footer(frame: &mut ratatui::Frame, area: Rect, state: &TuiState) {
    let task_count = state.tasks.len();
    let running = state
        .tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Running || t.status == TaskStatus::Verifying)
        .count();
    let help = state.status.clone();

    let paragraph = Paragraph::new(Line::from(vec![
        Span::styled(
            format!(" {task_count} task(s), {running} active "),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(format!(" {help} "), Style::default()),
    ]))
    .block(Block::default().borders(Borders::TOP));

    frame.render_widget(paragraph, area);
}

fn render_status_line(frame: &mut ratatui::Frame, area: Rect, state: &TuiState) {
    let text = if let Some(task) = state.current() {
        format!(
            "{}  {}  {} tool call(s)  {} step(s)",
            render::status_label(task.status),
            render::short_id(&task.id),
            task.tool_calls,
            task.steps
        )
    } else {
        "no task selected".to_string()
    };

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            text,
            Style::default().fg(Color::DarkGray),
        )))
        .block(Block::default()),
        area,
    );
}

fn render_prompt(frame: &mut ratatui::Frame, area: Rect, state: &TuiState) {
    let label = state.prompt.clone().unwrap_or_default();
    let width = (area.width.saturating_sub(4)).min(80);
    let height = 3u16;
    let x = area.x + 2;
    let y = area.y + area.height.saturating_sub(height + 2);
    let prompt_area = Rect::new(x, y, width, height);

    // Clear the area so the prompt is readable over the interface.
    frame.render_widget(ratatui::widgets::Clear, prompt_area);

    let paragraph = Paragraph::new(Line::from(vec![
        Span::styled(
            format!(" {label} "),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(state.input.clone(), Style::default().fg(Color::White)),
        Span::styled("█", Style::default().fg(Color::Gray)),
    ]))
    .block(Block::default().borders(Borders::ALL).title(" Input "));

    frame.render_widget(paragraph, prompt_area);
}
