//! A job's log file, shared by the supervisor and its two output pumps.
//!
//! Besides the child's output, ptrack writes a few lines of its own: a header
//! when the job starts, heartbeats while it is silent, and a footer when it
//! ends. They all start with `# `, so the log is never empty and always says
//! where the job ended up, even when the command printed nothing (or sent its
//! output elsewhere with `> file`). ptrack's own lines never become the job's
//! `last_line`.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use crate::util::now_ms;

/// What `last_line` says for a job that printed nothing at all.
pub const NO_OUTPUT: &str = "(no output)";

pub struct Output {
    inner: Mutex<Inner>,
}

struct Inner {
    file: File,
    /// Whether the log currently ends with a newline (or is empty), so
    /// ptrack's own lines are never glued to a half-printed line.
    at_line_start: bool,
    /// Bytes and newlines the child printed (ptrack's own lines not counted).
    bytes: u64,
    newlines: u64,
    last_output_ms: Option<u64>,
    last_line: Option<String>,
}

impl Output {
    pub fn open(path: &Path) -> io::Result<Output> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Output {
            inner: Mutex::new(Inner {
                file,
                at_line_start: true,
                bytes: 0,
                newlines: 0,
                last_output_ms: None,
                last_line: None,
            }),
        })
    }

    // A pump thread that panicked must not take the log down with it.
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Appends a chunk of the child's output. `newest` is the last meaningful
    /// line in it, if any.
    pub fn child_wrote(&self, chunk: &[u8], newest: Option<String>) {
        if chunk.is_empty() {
            return;
        }
        let mut inner = self.lock();
        let _ = inner.file.write_all(chunk);
        inner.bytes += chunk.len() as u64;
        inner.newlines += chunk.iter().filter(|&&b| b == b'\n').count() as u64;
        inner.at_line_start = chunk.ends_with(b"\n");
        inner.last_output_ms = Some(now_ms());
        if newest.is_some() {
            inner.last_line = newest;
        }
    }

    /// Appends ptrack's own lines (each should start with `# `), on a line
    /// of their own.
    pub fn note(&self, lines: &[String]) {
        let mut inner = self.lock();
        let mut text = String::new();
        if !inner.at_line_start {
            text.push('\n');
        }
        for line in lines {
            text.push_str(line);
            text.push('\n');
        }
        let _ = inner.file.write_all(text.as_bytes());
        inner.at_line_start = true;
    }

    pub fn last_line(&self) -> Option<String> {
        self.lock().last_line.clone()
    }

    /// When the child last printed anything; `None` if it never did.
    pub fn last_output_ms(&self) -> Option<u64> {
        self.lock().last_output_ms
    }

    /// Lines the child printed, counting an unfinished last line; `None` if
    /// it printed nothing at all.
    pub fn line_count(&self) -> Option<u64> {
        let inner = self.lock();
        (inner.bytes > 0).then(|| inner.newlines + u64::from(!inner.at_line_start))
    }
}

/// Prefixes of the lines ptrack itself writes into a log.
const OWN_PREFIXES: &[&str] = &[
    "# ptrack ",
    "# cmd: ",
    "#      ",
    "# shell: ",
    "# cwd: ",
    "# pid: ",
    "# started: ",
    "# ended: ",
    "# [ptrack] ",
];

pub fn is_own_line(line: &str) -> bool {
    OWN_PREFIXES.iter().any(|p| line.starts_with(p))
}

/// The last `n` lines the command printed, without ptrack's own lines. Used
/// where only the output matters, such as the lines shown after a failure.
pub fn last_output_lines(path: &Path, n: usize) -> io::Result<Vec<String>> {
    // ptrack adds a few lines at the start and end, plus heartbeats while
    // the job was silent; reading a little more keeps `n` real lines.
    let mut lines = crate::util::last_lines(path, n.saturating_mul(2) + 16)?;
    lines.retain(|l| !is_own_line(l));
    let skip = lines.len().saturating_sub(n);
    Ok(lines.split_off(skip))
}

/// `# cmd: ...` for a command that may span several lines (a shell script),
/// with every further line still marked as ptrack's own.
pub fn command_lines(command: &str) -> Vec<String> {
    command
        .lines()
        .enumerate()
        .map(|(i, l)| {
            let l = l.trim_end_matches('\r');
            if i == 0 {
                format!("# cmd: {l}")
            } else {
                format!("#      {l}")
            }
        })
        .collect()
}

/// `1 line`, `12 lines`, or `none`.
pub fn describe_lines(count: Option<u64>) -> String {
    match count {
        None => "none".into(),
        Some(1) => "1 line".into(),
        Some(n) => format!("{n} lines"),
    }
}

/// Elapsed time as a clock: `04:07` (minutes may pass 59).
pub fn clock_mm_ss(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}", s / 60, s % 60)
}

/// Elapsed time as a clock: `00:05:30`.
pub fn clock_hh_mm_ss(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_log() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("ptrack-output-{}.log", crate::util::new_id()))
    }

    #[test]
    fn own_lines_start_on_a_fresh_line_and_are_not_output() {
        let path = temp_log();
        let out = Output::open(&path).unwrap();
        assert_eq!(out.line_count(), None);
        out.note(&["# ptrack abc".into()]);
        out.child_wrote(b"50%", Some("50%".into()));
        out.note(&["# [ptrack] heartbeat".into()]);
        out.child_wrote(b"done\n", Some("done".into()));
        assert_eq!(out.line_count(), Some(1));
        assert_eq!(out.last_line().as_deref(), Some("done"));
        drop(out);
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(text, "# ptrack abc\n50%\n# [ptrack] heartbeat\ndone\n");
    }

    #[test]
    fn an_unfinished_last_line_still_counts() {
        let path = temp_log();
        let out = Output::open(&path).unwrap();
        out.child_wrote(b"a\nb", None);
        assert_eq!(out.line_count(), Some(2));
        drop(out);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn output_lines_leave_out_ptracks_own() {
        let path = temp_log();
        std::fs::write(
            &path,
            "# ptrack abc
# cmd: x
one
# [ptrack] 00:00:30 running
two
# ended: now  exit=1
",
        )
        .unwrap();
        let lines = last_output_lines(&path, 20).unwrap();
        let last = last_output_lines(&path, 1).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(lines, vec!["one", "two"]);
        assert_eq!(last, vec!["two"]);
    }

    #[test]
    fn helpers_format_as_documented() {
        assert_eq!(
            command_lines("echo a\r\necho b"),
            vec!["# cmd: echo a", "#      echo b"]
        );
        assert_eq!(describe_lines(None), "none");
        assert_eq!(describe_lines(Some(1)), "1 line");
        assert_eq!(describe_lines(Some(7)), "7 lines");
        assert_eq!(clock_mm_ss(65_000), "01:05");
        assert_eq!(clock_mm_ss(3_725_000), "62:05");
        assert_eq!(clock_hh_mm_ss(330_000), "00:05:30");
    }
}
