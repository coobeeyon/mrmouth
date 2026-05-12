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
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Terminal;

use crate::events::{EventSink, MessageTarget, MrmouthEvent};
use crate::logger::DisplaySink;

const MAX_LINES: usize = 500;
const POLL_TIMEOUT: Duration = Duration::from_millis(50);

/// Messages sent to the TUI render thread.
enum TuiMsg {
    Line { pane: String, text: String },
    SetRun(Option<String>),
    SetStage(String),
    Quit,
}

/// Owns the background render thread. When dropped, sends Quit and joins.
pub struct TuiHandle {
    tx: Sender<TuiMsg>,
    thread: Option<JoinHandle<()>>,
    cancelled: Arc<AtomicBool>,
}

/// Cloneable lifecycle-event renderer for the TUI.
#[derive(Clone)]
pub struct TuiEventSink {
    tx: Sender<TuiMsg>,
}

/// Cloneable sender that tags each line with a pane label.
#[derive(Clone)]
pub struct TuiSender {
    label: String,
    tx: Sender<TuiMsg>,
}

/// Internal state for all panes.
struct TuiState {
    /// Project name shown in the header.
    project: String,
    /// Current iteration label (e.g. "Run 3", "Task 2").
    run: Option<String>,
    /// Current stage shown in the header (e.g. "Agent", "Reviewer").
    stage: String,
    /// Ordered map of pane name -> ring buffer of lines.
    panes: Vec<(String, VecDeque<String>)>,
    /// Index of the currently active (visible) pane.
    active: usize,
    /// Vertical scroll offset (from bottom; 0 = auto-follow).
    scroll: u16,
}

impl TuiState {
    fn new(project: String) -> Self {
        Self {
            project,
            run: None,
            stage: String::new(),
            panes: Vec::new(),
            active: 0,
            scroll: 0,
        }
    }

    fn header_text(&self) -> String {
        let mut parts = vec![self.project.clone()];
        if let Some(ref label) = self.run {
            parts.push(label.clone());
        }
        if !self.stage.is_empty() {
            parts.push(self.stage.clone());
        }
        format!(" {} ", parts.join(" | "))
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
            for line in text.split('\n') {
                buf.push_back(line.to_string());
                if buf.len() > MAX_LINES {
                    buf.pop_front();
                }
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

    #[cfg(test)]
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
    pub fn try_start(project: &str) -> Option<Self> {
        if !io::stderr().is_terminal() {
            return None;
        }

        let (tx, rx) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancelled_clone = Arc::clone(&cancelled);
        let project = project.to_string();
        let thread = std::thread::spawn(move || {
            if let Err(e) = render_loop(rx, &cancelled_clone, project) {
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

    /// Create an event sink that renders mrmouth lifecycle events in the TUI.
    pub fn event_sink(&self) -> TuiEventSink {
        TuiEventSink {
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
}

impl EventSink for TuiEventSink {
    fn emit(&self, event: &MrmouthEvent) {
        match event {
            MrmouthEvent::Message { text, target, .. } => {
                let _ = self.tx.send(TuiMsg::Line {
                    pane: pane_for_target(*target).to_string(),
                    text: text.clone(),
                });
            }
            MrmouthEvent::StageChanged { stage } => {
                let _ = self.tx.send(TuiMsg::SetStage(stage.clone()));
            }
            MrmouthEvent::RunLabel { name, value } if is_header_run_label(name) => {
                let run = if value.is_empty() {
                    None
                } else {
                    Some(value.clone())
                };
                let _ = self.tx.send(TuiMsg::SetRun(run));
            }
            MrmouthEvent::TaskSelected { item_id, .. } => {
                let _ = self.tx.send(TuiMsg::SetRun(Some(item_id.clone())));
            }
            _ => {}
        }
    }
}

fn pane_for_target(target: MessageTarget) -> &'static str {
    match target {
        MessageTarget::Agent => "AGENT SESSION",
        MessageTarget::Decider => "DECIDER",
        MessageTarget::Reviewer => "REVIEWER",
        MessageTarget::Shipper => "SHIPPER",
        MessageTarget::System => "SYSTEM",
    }
}

fn is_header_run_label(name: &str) -> bool {
    matches!(name, "run" | "task" | "session")
}

impl DisplaySink for TuiSender {
    fn display_line(&self, line: &str) {
        self.send_line(line);
    }

    fn supports_color(&self) -> bool {
        true
    }
}

/// Main render loop running on the background thread.
fn render_loop(rx: Receiver<TuiMsg>, cancelled: &AtomicBool, project: String) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stderr = io::stderr();
    execute!(stderr, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stderr);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    let mut state = TuiState::new(project);
    let mut needs_redraw = true;

    loop {
        // Drain all pending messages (non-blocking after the first)
        loop {
            match rx.try_recv() {
                Ok(TuiMsg::Line { pane, text }) => {
                    state.push_line(&pane, text);
                    needs_redraw = true;
                }
                Ok(TuiMsg::SetRun(run)) => {
                    state.run = run;
                    needs_redraw = true;
                }
                Ok(TuiMsg::SetStage(stage)) => {
                    state.stage = stage;
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

fn draw(terminal: &mut Terminal<CrosstermBackend<io::Stderr>>, state: &TuiState) -> io::Result<()> {
    terminal.draw(|frame| {
        let has_multiple = state.panes.len() > 1;
        let footer_height = if has_multiple { 1 } else { 0 };

        let chunks = Layout::vertical([
            Constraint::Length(1),             // header
            Constraint::Min(1),                // body
            Constraint::Length(footer_height), // footer (only if >1 pane)
        ])
        .split(frame.area());

        // Header
        let header = Paragraph::new(Line::from(state.header_text()))
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

        let visible: Vec<Line<'_>> = text_lines.get(start..end).unwrap_or_default().to_vec();

        let body = Paragraph::new(visible)
            .block(Block::default().borders(Borders::NONE))
            .wrap(Wrap { trim: false });
        frame.render_widget(body, chunks[1]);

        // Footer (only if multiple panes)
        if has_multiple {
            let pane_labels: Vec<String> = state
                .panes
                .iter()
                .enumerate()
                .map(|(i, (name, _))| {
                    if i == state.active {
                        format!("[{name}]")
                    } else {
                        name.clone()
                    }
                })
                .collect();
            let footer_text = format!(" {}  Tab=next  q=quit", pane_labels.join("  "));
            let footer = Paragraph::new(Line::from(footer_text))
                .style(Style::default().fg(Color::Black).bg(Color::DarkGray));
            frame.render_widget(footer, chunks[2]);
        }
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{MessageLevel, MessageTarget};

    #[test]
    fn push_line_splits_on_newline() {
        let mut state = TuiState::new("test".into());
        state.push_line("test", "line1\nline2\nline3".to_string());
        let lines = state.active_lines();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "line1");
        assert_eq!(lines[1], "line2");
        assert_eq!(lines[2], "line3");
    }

    #[test]
    fn push_line_strips_carriage_returns() {
        let mut state = TuiState::new("test".into());
        state.push_line("test", "hello\r\nworld\r".to_string());
        let lines = state.active_lines();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "hello");
        assert_eq!(lines[1], "world");
    }

    #[test]
    fn push_line_single_line_no_split() {
        let mut state = TuiState::new("test".into());
        state.push_line("test", "just one line".to_string());
        let lines = state.active_lines();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], "just one line");
    }

    #[test]
    fn push_line_respects_max_lines() {
        let mut state = TuiState::new("test".into());
        // Push more than MAX_LINES individual lines
        for i in 0..MAX_LINES + 50 {
            state.push_line("test", format!("line {i}"));
        }
        assert_eq!(state.active_lines().len(), MAX_LINES);
        // Oldest lines should have been evicted
        assert_eq!(state.active_lines()[0], "line 50");
    }

    #[test]
    fn push_line_multiline_counts_toward_max() {
        let mut state = TuiState::new("test".into());
        // Fill to near capacity
        for i in 0..MAX_LINES - 2 {
            state.push_line("test", format!("line {i}"));
        }
        // Push a multi-line string that would exceed MAX_LINES
        state.push_line("test", "a\nb\nc\nd\ne".to_string());
        assert_eq!(state.active_lines().len(), MAX_LINES);
    }

    #[test]
    fn push_line_resets_scroll_on_active_pane() {
        let mut state = TuiState::new("test".into());
        state.push_line("pane1", "hello".to_string());
        state.scroll = 10;
        state.push_line("pane1", "world".to_string());
        assert_eq!(state.scroll, 0);
    }

    #[test]
    fn next_pane_cycles() {
        let mut state = TuiState::new("test".into());
        state.push_line("A", "a".to_string());
        state.push_line("B", "b".to_string());
        // Active should be "B" (auto-switched to newest)
        assert_eq!(state.active_name(), "B");
        state.next_pane();
        assert_eq!(state.active_name(), "A");
        state.next_pane();
        assert_eq!(state.active_name(), "B");
    }

    #[test]
    fn event_sink_routes_message_targets_to_panes() {
        let (tx, rx) = mpsc::channel();
        let sink = TuiEventSink { tx };

        sink.emit(&MrmouthEvent::Message {
            level: MessageLevel::Info,
            text: "review output".to_string(),
            target: MessageTarget::Reviewer,
        });

        match rx.try_recv().unwrap() {
            TuiMsg::Line { pane, text } => {
                assert_eq!(pane, "REVIEWER");
                assert_eq!(text, "review output");
            }
            _ => panic!("expected line message"),
        }
    }

    #[test]
    fn event_sink_updates_header_from_stage_and_run_events() {
        let (tx, rx) = mpsc::channel();
        let sink = TuiEventSink { tx };

        sink.emit(&MrmouthEvent::StageChanged {
            stage: "Reviewer".to_string(),
        });
        sink.emit(&MrmouthEvent::RunLabel {
            name: "run".to_string(),
            value: "Run 2".to_string(),
        });

        match rx.try_recv().unwrap() {
            TuiMsg::SetStage(stage) => assert_eq!(stage, "Reviewer"),
            _ => panic!("expected stage message"),
        }
        match rx.try_recv().unwrap() {
            TuiMsg::SetRun(run) => assert_eq!(run, Some("Run 2".to_string())),
            _ => panic!("expected run message"),
        }
    }

    #[test]
    fn event_sink_ignores_non_header_run_labels() {
        let (tx, rx) = mpsc::channel();
        let sink = TuiEventSink { tx };

        sink.emit(&MrmouthEvent::RunLabel {
            name: "branch".to_string(),
            value: "feature".to_string(),
        });

        assert!(rx.try_recv().is_err());
    }
}
