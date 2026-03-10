use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::tui::TuiSender;

const WIDTH: usize = 80;

fn make_banner_str(label: &str) -> String {
    let border = "#".repeat(WIDTH);
    let empty = format!("##{}##", " ".repeat(WIDTH - 4));
    // "##  " (4) + label padded to (WIDTH-6) + "##" (2) = WIDTH
    let text = format!("##  {:<width$}##", label, width = WIDTH - 6);
    format!("{border}\n{empty}\n{text}\n{empty}\n{border}")
}

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

    /// Clone this Logger with a different TUI pane label, sharing the same file writer.
    pub fn with_label(&self, label: &str) -> Self {
        Self {
            writer: Arc::clone(&self.writer),
            tui: self.tui.as_ref().map(|t| t.with_label(label)),
        }
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

    /// Print a 5-line stage banner to display and log file.
    pub fn banner(&self, label: &str) {
        self.log(&make_banner_str(label));
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

    /// Spawn a thread that tees child stdout → display (TUI or stdout) + log file.
    pub fn tee_stdout(&self, stdout: std::process::ChildStdout) -> JoinHandle<()> {
        let writer = Arc::clone(&self.writer);
        let tui = self.tui.clone();
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                match &tui {
                    Some(sender) => sender.send_line(&line),
                    None => println!("{line}"),
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

/// Print a 5-line stage banner via `logger` if Some, otherwise to stderr.
pub fn banner(logger: Option<&Logger>, label: &str) {
    let s = make_banner_str(label);
    match logger {
        Some(l) => l.log(&s),
        None => eprintln!("{s}"),
    }
}
