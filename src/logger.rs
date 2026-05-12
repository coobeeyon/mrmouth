use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, IsTerminal, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

type SharedWriter = Arc<Mutex<BufWriter<File>>>;

/// Display-only output destination for human-readable log lines.
pub trait DisplaySink: Send + Sync {
    fn display_line(&self, line: &str);

    fn supports_color(&self) -> bool {
        false
    }
}

/// Cloneable display sink reference shared by loggers and stream renderers.
#[derive(Clone)]
pub struct DisplaySinkHandle {
    sink: Arc<dyn DisplaySink>,
}

impl DisplaySinkHandle {
    pub fn new<S>(sink: S) -> Self
    where
        S: DisplaySink + 'static,
    {
        Self {
            sink: Arc::new(sink),
        }
    }

    pub fn stderr() -> Self {
        Self::new(StderrDisplaySink)
    }

    pub fn display_line(&self, line: &str) {
        self.sink.display_line(line);
    }

    pub fn supports_color(&self) -> bool {
        self.sink.supports_color()
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct StderrDisplaySink;

impl DisplaySink for StderrDisplaySink {
    fn display_line(&self, line: &str) {
        eprintln!("{line}");
    }

    fn supports_color(&self) -> bool {
        std::io::stderr().is_terminal()
    }
}

/// Tees all output to both a display target and a log file.
#[derive(Clone)]
pub struct Logger {
    writer: SharedWriter,
    display: DisplaySinkHandle,
    custom_display: bool,
}

impl Logger {
    pub fn new(path: &Path) -> std::io::Result<Self> {
        let file = File::create(path)?;
        Ok(Self {
            writer: Arc::new(Mutex::new(BufWriter::new(file))),
            display: DisplaySinkHandle::stderr(),
            custom_display: false,
        })
    }

    /// Create a Logger that routes display output to a display sink.
    pub fn with_display_sink<S>(path: &Path, display: S) -> std::io::Result<Self>
    where
        S: DisplaySink + 'static,
    {
        Self::with_display_handle(path, DisplaySinkHandle::new(display))
    }

    /// Create a Logger that routes display output to an existing display sink.
    pub fn with_display_handle(path: &Path, display: DisplaySinkHandle) -> std::io::Result<Self> {
        let file = File::create(path)?;
        Ok(Self {
            writer: Arc::new(Mutex::new(BufWriter::new(file))),
            display,
            custom_display: true,
        })
    }

    /// Write `msg` to display and to the log file.
    pub fn log(&self, msg: &str) {
        self.display_line(msg);
        self.write_file(msg);
    }

    /// Write `msg` to display without writing to the log file.
    /// Used for stream-formatted output that's already logged separately.
    pub fn display(&self, msg: &str) {
        self.display_line(msg);
    }

    /// Write `msg` to the log file only (use when it's already printed elsewhere).
    pub fn log_file_only(&self, msg: &str) {
        self.write_file(msg);
    }

    fn display_line(&self, msg: &str) {
        self.display.display_line(msg);
    }

    fn write_file(&self, msg: &str) {
        if let Ok(mut w) = self.writer.lock() {
            let _ = writeln!(w, "{msg}");
        }
    }

    /// Spawn a thread that tees child stderr to display and the log file.
    /// Join the returned handle before continuing to ensure all output is captured.
    pub fn tee_stderr(&self, stderr: std::process::ChildStderr) -> JoinHandle<()> {
        let writer = Arc::clone(&self.writer);
        let display = self.display.clone();
        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                display.display_line(&line);
                if let Ok(mut w) = writer.lock() {
                    let _ = writeln!(w, "{line}");
                }
            }
        })
    }

    /// Returns true when display is routed through a caller-provided sink.
    pub fn has_custom_display(&self) -> bool {
        self.custom_display
    }

    /// Clone the caller-provided display sink, if present.
    pub fn display_sink(&self) -> Option<DisplaySinkHandle> {
        self.custom_display.then(|| self.display.clone())
    }

    pub fn display_supports_color(&self) -> bool {
        self.display.supports_color()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct RecordingDisplaySink {
        lines: Arc<Mutex<Vec<String>>>,
    }

    impl DisplaySink for RecordingDisplaySink {
        fn display_line(&self, line: &str) {
            self.lines.lock().unwrap().push(line.to_string());
        }
    }

    #[test]
    fn logger_routes_display_through_sink_and_preserves_file_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.log");
        let sink = RecordingDisplaySink::default();
        let lines = Arc::clone(&sink.lines);
        let logger = Logger::with_display_sink(&path, sink).unwrap();

        logger.log("hello");
        logger.flush();

        assert_eq!(lines.lock().unwrap().as_slice(), ["hello"]);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "hello\n");
    }

    #[test]
    fn display_only_does_not_change_file_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.log");
        let sink = RecordingDisplaySink::default();
        let lines = Arc::clone(&sink.lines);
        let logger = Logger::with_display_sink(&path, sink).unwrap();

        logger.display("visible");
        logger.flush();

        assert_eq!(lines.lock().unwrap().as_slice(), ["visible"]);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "");
    }
}
