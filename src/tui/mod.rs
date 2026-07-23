//! Terminal User Interface for rexeb

use std::io;
use std::time::Duration;

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, Gauge, List, ListItem, Paragraph},
    Terminal,
};
use tokio::sync::mpsc;

use crate::error::Result;

/// Events that the worker can send to the TUI
#[derive(Debug, Clone)]
pub enum ProgressEvent {
    /// Set overall progress (0.0 - 1.0)
    Progress(f64),
    /// Set the current status message
    Status(String),
    /// Append a log message
    Log(String),
    /// Signal that work is complete
    Done,
    /// Signal an error
    Error(String),
}

/// TUI application state
pub struct App {
    /// Current progress (0.0 - 1.0)
    pub progress: f64,
    /// Current status message
    pub status: String,
    /// Logs
    pub logs: Vec<String>,
}

impl App {
    /// Create a new app state
    pub fn new() -> Self {
        Self {
            progress: 0.0,
            status: String::from("Ready"),
            logs: Vec::new(),
        }
    }

    /// Process a single progress event
    pub fn handle_event(&mut self, event: ProgressEvent) {
        match event {
            ProgressEvent::Progress(p) => self.progress = p.clamp(0.0, 1.0),
            ProgressEvent::Status(s) => self.status = s,
            ProgressEvent::Log(msg) => {
                self.logs.push(msg);
                if self.logs.len() > 100 {
                    self.logs.remove(0);
                }
            }
            ProgressEvent::Done => {
                self.progress = 1.0;
                self.status = "Complete".to_string();
            }
            ProgressEvent::Error(e) => {
                self.logs.push(format!("ERROR: {}", e));
                self.status = "Error".to_string();
            }
        }
    }
}

/// Run the TUI, polling `rx` for progress events until `Done` or `Error` is received.
pub async fn run_tui(
    mut app: App,
    tick_rate: Duration,
    mut rx: mpsc::Receiver<ProgressEvent>,
) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let res = run_app(&mut terminal, &mut app, tick_rate, &mut rx).await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("{:?}", err);
    }

    Ok(())
}

async fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    tick_rate: Duration,
    rx: &mut mpsc::Receiver<ProgressEvent>,
) -> io::Result<()> {
    let tick_interval = tokio::time::interval(tick_rate);
    tokio::pin!(tick_interval);

    loop {
        terminal.draw(|f| ui(f, app))?;

        tokio::select! {
            // Process UI events (keyboard input)
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                if crossterm::event::poll(Duration::from_millis(0))? {
                    if let Event::Key(key) = event::read()? {
                        if let KeyCode::Char('q') = key.code {
                            return Ok(());
                        }
                    }
                }
            }
            // Receive progress events from the worker
            event = rx.recv() => {
                match event {
                    Some(evt) => {
                        let is_terminal = matches!(&evt, ProgressEvent::Done | ProgressEvent::Error(_));
                        app.handle_event(evt);
                        if is_terminal {
                            // Draw one final frame so the user sees the completed state
                            terminal.draw(|f| ui(f, app))?;
                            // Wait a moment so they can see the result before quitting
                            tokio::time::sleep(Duration::from_secs(2)).await;
                            return Ok(());
                        }
                    }
                    None => return Ok(()), // Channel closed
                }
            }
        }
    }
}

fn ui<B: Backend>(f: &mut ratatui::Frame<B>, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints(
            [
                Constraint::Length(3), // Title
                Constraint::Length(3), // Progress
                Constraint::Min(10),   // Logs/Details
                Constraint::Length(3), // Status
            ]
            .as_ref(),
        )
        .split(f.size());

    // Title
    let title = Paragraph::new(Spans::from(vec![
        Span::styled("Rexeb - Smarter Package Converter", Style::default().add_modifier(Modifier::BOLD)),
    ]))
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // Progress
    let gauge = Gauge::default()
        .block(Block::default().title("Progress").borders(Borders::ALL))
        .gauge_style(Style::default().fg(Color::Cyan))
        .ratio(app.progress);
    f.render_widget(gauge, chunks[1]);

    // Logs
    let logs: Vec<ListItem> = app
        .logs
        .iter()
        .rev()
        .map(|m| ListItem::new(Span::raw(m)))
        .collect();
    let logs_list = List::new(logs)
        .block(Block::default().title("Logs").borders(Borders::ALL));
    f.render_widget(logs_list, chunks[2]);

    // Status
    let status = Paragraph::new(app.status.as_str())
        .block(Block::default().title("Status").borders(Borders::ALL));
    f.render_widget(status, chunks[3]);
}