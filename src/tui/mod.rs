//! Terminal User Interface for rexeb

use std::io;
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

use crate::error::Result;

/// TUI application state
pub struct App {
    /// List of items to process
    pub items: Vec<String>,
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
            items: Vec::new(),
            progress: 0.0,
            status: String::new(),
            logs: Vec::new(),
        }
    }

    /// Add a log message
    pub fn log(&mut self, message: impl Into<String>) {
        self.logs.push(message.into());
        if self.logs.len() > 100 {
            self.logs.remove(0);
        }
    }
}

/// Run the TUI
pub fn run_tui<F>(mut app: App, tick_rate: std::time::Duration, mut worker: F) -> Result<()>
where
    F: FnMut(&mut App) -> Result<bool>, // returns true when done
{
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Run loop
    let res = run_app(&mut terminal, &mut app, tick_rate, &mut worker);

    // Restore terminal
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

fn run_app<B: Backend, F>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    tick_rate: std::time::Duration,
    worker: &mut F,
) -> io::Result<()>
where
    F: FnMut(&mut App) -> Result<bool>,
{
    let mut last_tick = std::time::Instant::now();
    loop {
        terminal.draw(|f| ui(f, app))?;

        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| std::time::Duration::from_secs(0));

        if crossterm::event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if let KeyCode::Char('q') = key.code {
                    return Ok(());
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            match worker(app) {
                Ok(done) => {
                    if done {
                        return Ok(());
                    }
                }
                Err(e) => {
                    app.log(format!("Error: {}", e));
                }
            }
            last_tick = std::time::Instant::now();
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