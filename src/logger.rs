use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::tui::TuiSender;

type SharedWriter = Arc<Mutex<BufWriter<File>>>;

/// Tees all output to both a display target (TUI pane or terminal) and a log file.
#[derive(Clone)]
pub struct Logger {
    writer: SharedWriter,
    tui: Option<TuiSender>,
}

impl Logger {
    pub fn new(path: &Path) -> std::io::Result<Self> {
        let file = File::create(path)?;
        Ok(Self {
            writer: Arc::new(Mutex::new(BufWriter::new(file))),
            tui: None,
        })
    }

    /// Create a Logger that routes display output to a TUI pane.
    pub fn with_tui(path: &Path, tui: TuiSender) -> std::io::Result<Self> {
        let file = File::create(path)?;
        Ok(Self {
            writer: Arc::new(Mutex::new(BufWriter::new(file))),
            tui: Some(tui),
        })
    }

    /// Write `msg` to display (TUI or stderr) and to the log file.
    pub fn log(&self, msg: &str) {
        self.display_line(msg);
        self.write_file(msg);
    }

    /// Write `msg` to display (TUI or stdout) without writing to the log file.
    /// Used for stream-formatted output that's already logged separately.
    pub fn display(&self, msg: &str) {
        self.display_line(msg);
    }

    /// Write `msg` to the log file only (use when it's already printed elsewhere).
    pub fn log_file_only(&self, msg: &str) {
        self.write_file(msg);
    }

    fn display_line(&self, msg: &str) {
        match &self.tui {
            Some(sender) => sender.send_line(msg),
            None => eprintln!("{msg}"),
        }
    }

    fn write_file(&self, msg: &str) {
        if let Ok(mut w) = self.writer.lock() {
            let _ = writeln!(w, "{msg}");
        }
    }

    /// Spawn a thread that tees child stderr → display (TUI or stderr) + log file.
    /// Join the returned handle before continuing to ensure all output is captured.
    pub fn tee_stderr(&self, stderr: std::process::ChildStderr) -> JoinHandle<()> {
        let writer = Arc::clone(&self.writer);
        let tui = self.tui.clone();
        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                match &tui {
                    Some(sender) => sender.send_line(&line),
                    None => eprintln!("{line}"),
                }
                if let Ok(mut w) = writer.lock() {
                    let _ = writeln!(w, "{line}");
                }
            }
        })
    }

    /// Returns true if this Logger has a TUI sender attached.
    pub fn has_tui(&self) -> bool {
        self.tui.is_some()
    }

    /// Get a reference to the TUI sender, if present.
    pub fn tui_sender(&self) -> Option<&TuiSender> {
        self.tui.as_ref()
    }

    pub fn flush(&self) {
        if let Ok(mut w) = self.writer.lock() {
            let _ = w.flush();
        }
    }
}

/// Log to `logger` if Some, otherwise print to stderr.
pub fn log(logger: Option<&Logger>, msg: &str) {
    match logger {
        Some(l) => l.log(msg),
        None => eprintln!("{msg}"),
    }
}
