use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub const TAIL_LINES: usize = 25;

/// Post-mortem info extracted from a top-level error, printed to stderr
/// after the TUI has been dropped. Converts "error: <count>" into a full
/// "error: <msg> / exit <code> / log: <abs path> / <tail>" block so the
/// user can tell at a glance what the container said before it died.
pub struct FailureDebrief {
    pub message: String,
    pub exit_code: Option<i32>,
    pub log_path: Option<PathBuf>,
    /// Pre-rendered per-attempt summary lines (used by TooManyFailures).
    pub attempt_lines: Vec<String>,
}

impl FailureDebrief {
    pub fn new(message: String) -> Self {
        Self { message, exit_code: None, log_path: None, attempt_lines: Vec::new() }
    }

    /// Print the debrief to stderr. Call after the TUI has been dropped, so
    /// the alt-screen teardown doesn't wipe the output.
    pub fn print(&self) {
        eprintln!("error: {}", self.message);
        if let Some(code) = self.exit_code {
            eprintln!("exit code: {code}");
        }
        if let Some(path) = &self.log_path {
            let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            eprintln!("log: {}", abs.display());
        }
        if !self.attempt_lines.is_empty() {
            eprintln!();
            eprintln!("Per-attempt summary:");
            for l in &self.attempt_lines {
                eprintln!("  {l}");
            }
        }
        if let Some(path) = &self.log_path {
            let tail = read_log_tail_lines(path, TAIL_LINES);
            if !tail.is_empty() {
                eprintln!();
                eprintln!("--- last {} lines of log ---", tail.len());
                for line in &tail {
                    eprintln!("{line}");
                }
                eprintln!("--- end of log ---");
            }
        }
    }
}

/// Read up to `n` lines from the end of `path`. Returns empty on any I/O error
/// so callers can fall back to the rest of the debrief without crashing.
pub fn read_log_tail_lines(path: &Path, n: usize) -> Vec<String> {
    if n == 0 { return Vec::new(); }
    let Ok(file) = File::open(path) else { return Vec::new(); };
    let reader = BufReader::new(file);
    let mut buf: VecDeque<String> = VecDeque::with_capacity(n);
    for line in reader.lines().map_while(Result::ok) {
        if buf.len() == n {
            buf.pop_front();
        }
        buf.push_back(line);
    }
    buf.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_returns_empty_for_missing_file() {
        let path = PathBuf::from("/nonexistent/path/to/log-iyie");
        assert!(read_log_tail_lines(&path, 25).is_empty());
    }

    #[test]
    fn tail_returns_all_lines_when_fewer_than_n() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("short.log");
        std::fs::write(&path, "one\ntwo\nthree\n").unwrap();
        let lines = read_log_tail_lines(&path, 25);
        assert_eq!(lines, vec!["one", "two", "three"]);
    }

    #[test]
    fn tail_returns_last_n_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("long.log");
        let content: String = (0..100).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&path, content).unwrap();
        let lines = read_log_tail_lines(&path, 5);
        assert_eq!(lines, vec!["line 95", "line 96", "line 97", "line 98", "line 99"]);
    }

    #[test]
    fn tail_handles_zero_n() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.log");
        std::fs::write(&path, "a\nb\n").unwrap();
        assert!(read_log_tail_lines(&path, 0).is_empty());
    }

    #[test]
    fn tail_handles_file_without_trailing_newline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nn.log");
        std::fs::write(&path, "alpha\nbeta").unwrap();
        let lines = read_log_tail_lines(&path, 25);
        assert_eq!(lines, vec!["alpha", "beta"]);
    }

    #[test]
    fn debrief_new_has_no_optional_fields() {
        let d = FailureDebrief::new("boom".into());
        assert_eq!(d.message, "boom");
        assert!(d.exit_code.is_none());
        assert!(d.log_path.is_none());
        assert!(d.attempt_lines.is_empty());
    }
}
