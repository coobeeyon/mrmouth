use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

const WIDTH: usize = 80;

fn make_banner_str(label: &str) -> String {
    let border = "#".repeat(WIDTH);
    let empty = format!("##{}##", " ".repeat(WIDTH - 4));
    // "##  " (4) + label padded to (WIDTH-6) + "##" (2) = WIDTH
    let text = format!("##  {:<width$}##", label, width = WIDTH - 6);
    format!("{border}\n{empty}\n{text}\n{empty}\n{border}")
}

type SharedWriter = Arc<Mutex<BufWriter<File>>>;

/// Tees all output to both stderr/stdout (terminal) and a rotating log file.
#[derive(Clone)]
pub struct Logger {
    writer: SharedWriter,
}

impl Logger {
    pub fn new(path: &Path) -> std::io::Result<Self> {
        let file = File::create(path)?;
        Ok(Self {
            writer: Arc::new(Mutex::new(BufWriter::new(file))),
        })
    }

    /// Write `msg` to stderr and to the log file.
    pub fn log(&self, msg: &str) {
        eprintln!("{msg}");
        self.write_file(msg);
    }

    /// Write `msg` to the log file only (use when it's already printed elsewhere).
    pub fn log_file_only(&self, msg: &str) {
        self.write_file(msg);
    }

    fn write_file(&self, msg: &str) {
        if let Ok(mut w) = self.writer.lock() {
            let _ = writeln!(w, "{msg}");
        }
    }

    /// Print a 5-line stage banner to stderr and log file.
    pub fn banner(&self, label: &str) {
        self.log(&make_banner_str(label));
    }

    /// Spawn a thread that tees child stderr → terminal (stderr) + log file.
    /// Join the returned handle before continuing to ensure all output is captured.
    pub fn tee_stderr(&self, stderr: std::process::ChildStderr) -> JoinHandle<()> {
        let writer = Arc::clone(&self.writer);
        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                eprintln!("{line}");
                if let Ok(mut w) = writer.lock() {
                    let _ = writeln!(w, "{line}");
                }
            }
        })
    }

    /// Spawn a thread that tees child stdout → terminal (stdout) + log file.
    pub fn tee_stdout(&self, stdout: std::process::ChildStdout) -> JoinHandle<()> {
        let writer = Arc::clone(&self.writer);
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                println!("{line}");
                if let Ok(mut w) = writer.lock() {
                    let _ = writeln!(w, "{line}");
                }
            }
        })
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
