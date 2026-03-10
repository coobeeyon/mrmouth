use std::collections::VecDeque;
use std::io::{self, IsTerminal};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Terminal;

const MAX_LINES: usize = 500;
const POLL_TIMEOUT: Duration = Duration::from_millis(50);

/// Messages sent to the TUI render thread.
enum TuiMsg {
    Line { pane: String, text: String },
    Quit,
}

/// Owns the background render thread. When dropped, sends Quit and joins.
pub struct TuiHandle {
    tx: Sender<TuiMsg>,
    thread: Option<JoinHandle<()>>,
    cancelled: Arc<AtomicBool>,
}

/// Cloneable sender that tags each line with a pane label.
#[derive(Clone)]
pub struct TuiSender {
    label: String,
    tx: Sender<TuiMsg>,
}

/// Internal state for all panes.
struct TuiState {
    /// Ordered map of pane name -> ring buffer of lines.
    panes: Vec<(String, VecDeque<String>)>,
    /// Index of the currently active (visible) pane.
    active: usize,
    /// Vertical scroll offset (from bottom; 0 = auto-follow).
    scroll: u16,
}

impl TuiState {
    fn new() -> Self {
        Self {
            panes: Vec::new(),
            active: 0,
            scroll: 0,
        }
    }

    fn ensure_pane(&mut self, name: &str) {
        if !self.panes.iter().any(|(n, _)| n == name) {
            self.panes.push((name.to_string(), VecDeque::new()));
            // Auto-switch to newest pane
            self.active = self.panes.len() - 1;
        }
    }

    fn push_line(&mut self, pane: &str, text: String) {
        self.ensure_pane(pane);
        if let Some((_, buf)) = self.panes.iter_mut().find(|(n, _)| n == pane) {
            let text = text.replace('\r', "");
            buf.push_back(text);
            if buf.len() > MAX_LINES {
                buf.pop_front();
            }
        }
        // If the active pane received a line, reset scroll to follow
        if let Some((name, _)) = self.panes.get(self.active) {
            if name == pane {
                self.scroll = 0;
            }
        }
    }

    fn next_pane(&mut self) {
        if !self.panes.is_empty() {
            self.active = (self.active + 1) % self.panes.len();
            self.scroll = 0;
        }
    }

    fn active_name(&self) -> &str {
        self.panes
            .get(self.active)
            .map(|(n, _)| n.as_str())
            .unwrap_or("")
    }

    fn active_lines(&self) -> &VecDeque<String> {
        static EMPTY: VecDeque<String> = VecDeque::new();
        self.panes
            .get(self.active)
            .map(|(_, buf)| buf)
            .unwrap_or(&EMPTY)
    }
}

impl TuiHandle {
    /// Start the TUI if stderr is a TTY. Returns None if not a TTY.
    pub fn try_start() -> Option<Self> {
        if !io::stderr().is_terminal() {
            return None;
        }

        let (tx, rx) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancelled_clone = Arc::clone(&cancelled);
        let thread = std::thread::spawn(move || {
            if let Err(e) = render_loop(rx, &cancelled_clone) {
                // Best effort: restore terminal and print error
                let _ = disable_raw_mode();
                let _ = execute!(io::stderr(), LeaveAlternateScreen);
                eprintln!("TUI error: {e}");
            }
        });

        Some(Self {
            tx,
            thread: Some(thread),
            cancelled,
        })
    }

    /// Returns true if the user requested cancellation (q or Ctrl+C).
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// Returns a clone of the cancellation flag for use in watcher threads.
    pub fn cancelled_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancelled)
    }

    /// Create a TuiSender that tags lines with the given pane label.
    pub fn sender(&self, label: &str) -> TuiSender {
        TuiSender {
            label: label.to_string(),
            tx: self.tx.clone(),
        }
    }
}

impl Drop for TuiHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(TuiMsg::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl TuiSender {
    /// Send a line of text to this sender's pane.
    pub fn send_line(&self, line: &str) {
        let _ = self.tx.send(TuiMsg::Line {
            pane: self.label.clone(),
            text: line.to_string(),
        });
    }

    /// Create a new sender with a different pane label, sharing the same channel.
    pub fn with_label(&self, label: &str) -> Self {
        Self {
            label: label.to_string(),
            tx: self.tx.clone(),
        }
    }
}

/// Main render loop running on the background thread.
fn render_loop(rx: Receiver<TuiMsg>, cancelled: &AtomicBool) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stderr = io::stderr();
    execute!(stderr, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stderr);
    let mut terminal = Terminal::new(backend)?;
    let mut state = TuiState::new();
    let mut needs_redraw = true;

    loop {
        // Drain all pending messages (non-blocking after the first)
        loop {
            match rx.try_recv() {
                Ok(TuiMsg::Line { pane, text }) => {
                    state.push_line(&pane, text);
                    needs_redraw = true;
                }
                Ok(TuiMsg::Quit) => {
                    cleanup_terminal(&mut terminal)?;
                    return Ok(());
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    cleanup_terminal(&mut terminal)?;
                    return Ok(());
                }
            }
        }

        // Poll for keyboard events
        if event::poll(POLL_TIMEOUT)? {
            if let Event::Key(key) = event::read()? {
                match (key.code, key.modifiers) {
                    (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                        cancelled.store(true, Ordering::Relaxed);
                        cleanup_terminal(&mut terminal)?;
                        return Ok(());
                    }
                    (KeyCode::Tab, _) => {
                        state.next_pane();
                        needs_redraw = true;
                    }
                    (KeyCode::Up, _) => {
                        state.scroll = state.scroll.saturating_add(3);
                        needs_redraw = true;
                    }
                    (KeyCode::Down, _) => {
                        state.scroll = state.scroll.saturating_sub(3);
                        needs_redraw = true;
                    }
                    _ => {}
                }
            }
        }

        if needs_redraw {
            draw(&mut terminal, &state)?;
            needs_redraw = false;
        }
    }
}

fn cleanup_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stderr>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn draw(
    terminal: &mut Terminal<CrosstermBackend<io::Stderr>>,
    state: &TuiState,
) -> io::Result<()> {
    terminal.draw(|frame| {
        let has_multiple = state.panes.len() > 1;
        let footer_height = if has_multiple { 1 } else { 0 };

        let chunks = Layout::vertical([
            Constraint::Length(1),            // header
            Constraint::Min(1),               // body
            Constraint::Length(footer_height), // footer (only if >1 pane)
        ])
        .split(frame.area());

        // Header
        let active = state.active_name();
        let header_text = format!(" mrmouth | {} ", active.to_uppercase());
        let header = Paragraph::new(Line::from(header_text))
            .style(Style::default().fg(Color::Black).bg(Color::Cyan));
        frame.render_widget(header, chunks[0]);

        // Body — convert ANSI strings to ratatui Text
        let lines = state.active_lines();
        let body_height = chunks[1].height as usize;

        let text_lines: Vec<Line<'_>> = lines
            .iter()
            .flat_map(|s| {
                // ansi-to-tui parses ANSI escape codes into styled ratatui Lines
                match ansi_to_tui::IntoText::into_text(s) {
                    Ok(text) => text.lines,
                    Err(_) => vec![Line::raw(s.as_str())],
                }
            })
            .collect();

        // Apply scroll: scroll=0 means show the bottom
        let total = text_lines.len();
        let scroll_offset = state.scroll as usize;
        let end = total.saturating_sub(scroll_offset);
        let start = end.saturating_sub(body_height);

        let visible: Vec<Line<'_>> = text_lines.get(start..end)
            .unwrap_or_default()
            .to_vec();

        let body = Paragraph::new(visible)
            .block(Block::default().borders(Borders::NONE));
        frame.render_widget(body, chunks[1]);

        // Footer (only if multiple panes)
        if has_multiple {
            let pane_labels: Vec<String> = state
                .panes
                .iter()
                .enumerate()
                .map(|(i, (name, _))| {
                    if i == state.active {
                        format!("[{}]", name)
                    } else {
                        name.clone()
                    }
                })
                .collect();
            let footer_text = format!(
                " {}  Tab=next  q=quit",
                pane_labels.join("  ")
            );
            let footer = Paragraph::new(Line::from(footer_text))
                .style(Style::default().fg(Color::Black).bg(Color::DarkGray));
            frame.render_widget(footer, chunks[2]);
        }
    })?;
    Ok(())
}
