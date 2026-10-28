//! The registry: one JSON file per job plus one log file per job.
//!
//! Rule that keeps this lock-free: a job file has exactly one writer, the
//! supervisor that owns the job. Other commands only read it, and talk to the
//! supervisor through small marker files (`<id>.stop`, `<id>.paused`).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::platform;
use crate::util::{iso_utc, now_ms};

/// How often a supervisor refreshes its job file.
pub const HEARTBEAT_MS: u64 = 2_000;
/// A running job whose heartbeat is older than this, and whose supervisor is
/// gone, is reported as `lost`.
pub const STALE_MS: u64 = 10_000;
/// Past this age a heartbeat is not trusted even if the pid looks alive
/// (the pid may have been reused by an unrelated process).
const DEAD_MS: u64 = 60 * 60 * 1000;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Running,
    Paused,
    Passed,
    Failed,
    Cancelled,
    Lost,
}

impl State {
    pub fn is_active(self) -> bool {
        matches!(self, State::Running | State::Paused)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            State::Running => "running",
            State::Paused => "paused",
            State::Passed => "passed",
            State::Failed => "failed",
            State::Cancelled => "cancelled",
            State::Lost => "lost",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Job {
    pub id: String,
    pub label: Option<String>,
    pub command: Vec<String>,
    pub shell: bool,
    pub cwd: String,
    pub agent: Option<String>,
    pub session: Option<String>,
    /// The agent's id for the user prompt that led to this job.
    #[serde(default)]
    pub prompt_id: Option<String>,
    /// Start of that prompt's text, when the agent integration recorded it.
    #[serde(default)]
    pub prompt: Option<String>,
    /// Set when this job was started by `ptrack rerun` of another job.
    #[serde(default)]
    pub rerun_of: Option<String>,
    pub state: State,
    pub exit_code: Option<i32>,
    pub started_at: String,
    pub started_ms: u64,
    pub ended_at: Option<String>,
    pub ended_ms: Option<u64>,
    pub heartbeat_ms: u64,
    pub supervisor_pid: u32,
    pub pid: Option<u32>,
    pub log_path: String,
    pub last_line: Option<String>,
    pub error: Option<String>,
}

pub fn home() -> PathBuf {
    if let Some(p) = std::env::var_os("PTRACK_HOME") {
        return PathBuf::from(p);
    }
    #[cfg(windows)]
    if let Some(p) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(p).join("ptrack");
    }
    #[cfg(target_os = "macos")]
    if let Some(h) = std::env::var_os("HOME") {
        return PathBuf::from(h).join("Library/Application Support/ptrack");
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(x) = std::env::var_os("XDG_DATA_HOME") {
            return PathBuf::from(x).join("ptrack");
        }
        if let Some(h) = std::env::var_os("HOME") {
            return PathBuf::from(h).join(".local/share/ptrack");
        }
    }
    std::env::temp_dir().join("ptrack")
}

pub fn jobs_dir() -> PathBuf {
    home().join("jobs")
}

pub fn logs_dir() -> PathBuf {
    home().join("logs")
}

pub fn ensure_dirs() -> io::Result<()> {
    fs::create_dir_all(jobs_dir())?;
    fs::create_dir_all(logs_dir())
}

fn job_path(id: &str) -> PathBuf {
    jobs_dir().join(format!("{id}.json"))
}

/// Where agent integrations keep a short preview of each prompt, by prompt id.
pub fn prompts_dir() -> PathBuf {
    home().join("prompts")
}

pub fn prompt_preview(prompt_id: &str) -> Option<String> {
    let safe = prompt_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-');
    if !safe {
        return None;
    }
    fs::read_to_string(prompts_dir().join(format!("{prompt_id}.txt"))).ok()
}

pub fn stop_marker(id: &str) -> PathBuf {
    jobs_dir().join(format!("{id}.stop"))
}

pub fn paused_marker(id: &str) -> PathBuf {
    jobs_dir().join(format!("{id}.paused"))
}

/// Picks an id that is not used yet.
pub fn fresh_id() -> String {
    loop {
        let id = crate::util::new_id();
        if !job_path(&id).exists() {
            return id;
        }
    }
}

impl Job {
    pub fn new(id: String, command: Vec<String>, shell: bool) -> Job {
        let now = now_ms();
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        Job {
            log_path: logs_dir().join(format!("{id}.log")).display().to_string(),
            id,
            label: None,
            command,
            shell,
            cwd,
            agent: None,
            session: None,
            prompt_id: None,
            prompt: None,
            rerun_of: None,
            state: State::Running,
            exit_code: None,
            started_at: iso_utc(now),
            started_ms: now,
            ended_at: None,
            ended_ms: None,
            heartbeat_ms: now,
            supervisor_pid: std::process::id(),
            pid: None,
            last_line: None,
            error: None,
        }
    }

    pub fn finish(&mut self, state: State, exit_code: Option<i32>) {
        let now = now_ms();
        self.state = state;
        self.exit_code = exit_code;
        self.ended_ms = Some(now);
        self.ended_at = Some(iso_utc(now));
        self.heartbeat_ms = now;
    }

    /// Writes the job file atomically (temp file + rename). Retries briefly,
    /// because on Windows another program may hold the file open for a moment.
    pub fn save(&self) -> io::Result<()> {
        let path = job_path(&self.id);
        let tmp = jobs_dir().join(format!("{}.json.tmp", self.id));
        let data = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        let mut last_err = None;
        for _ in 0..20 {
            match fs::write(&tmp, &data).and_then(|_| fs::rename(&tmp, &path)) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    last_err = Some(e);
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
        Err(last_err.unwrap())
    }

    pub fn load(id: &str) -> io::Result<Job> {
        read_job(&job_path(id))
    }

    /// The state to show right now. A job file that says "running" is only
    /// believed while its supervisor is alive; otherwise the job is `lost`.
    pub fn effective_state(&self) -> State {
        if !self.state.is_active() {
            return self.state;
        }
        let age = now_ms().saturating_sub(self.heartbeat_ms);
        if age > DEAD_MS || (age > STALE_MS && !platform::pid_alive(self.supervisor_pid)) {
            return State::Lost;
        }
        if paused_marker(&self.id).exists() {
            State::Paused
        } else {
            State::Running
        }
    }

    pub fn duration_ms(&self) -> u64 {
        let end = match self.effective_state() {
            State::Lost => self.heartbeat_ms,
            s if s.is_active() => now_ms(),
            _ => self.ended_ms.unwrap_or(self.heartbeat_ms),
        };
        end.saturating_sub(self.started_ms)
    }

    /// Label if given, otherwise the command line.
    pub fn title(&self) -> String {
        match &self.label {
            Some(l) => l.clone(),
            None => self.command_line(),
        }
    }

    pub fn command_line(&self) -> String {
        self.command
            .iter()
            .map(|a| {
                if a.is_empty() || a.contains(char::is_whitespace) {
                    format!("\"{a}\"")
                } else {
                    a.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn read_job(path: &Path) -> io::Result<Job> {
    // A rename may be in flight on Windows; retry a few times.
    let mut last_err = None;
    for _ in 0..5 {
        match fs::read(path) {
            Ok(bytes) => {
                return serde_json::from_slice(&bytes)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(e),
            Err(e) => {
                last_err = Some(e);
                std::thread::sleep(Duration::from_millis(30));
            }
        }
    }
    Err(last_err.unwrap())
}

/// All jobs, newest first. Unreadable files are skipped.
pub fn list_all() -> Vec<Job> {
    let Ok(entries) = fs::read_dir(jobs_dir()) else {
        return Vec::new();
    };
    let mut jobs: Vec<Job> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| read_job(&p).ok())
        .collect();
    jobs.sort_by_key(|j| std::cmp::Reverse(j.started_ms));
    jobs
}

/// Finds a job by full id, unique id prefix, or `last` / no id for the newest.
pub fn resolve(query: Option<&str>) -> Result<Job, String> {
    let jobs = list_all();
    let query = query.unwrap_or("last");
    if query == "last" {
        return jobs
            .into_iter()
            .next()
            .ok_or_else(|| "no jobs recorded yet".to_string());
    }
    if let Some(j) = jobs.iter().find(|j| j.id == query) {
        return Ok(j.clone());
    }
    let mut matches: Vec<Job> = jobs
        .into_iter()
        .filter(|j| j.id.starts_with(query))
        .collect();
    match matches.len() {
        0 => Err(format!("no job matches '{query}'")),
        1 => Ok(matches.remove(0)),
        n => Err(format!("'{query}' matches {n} jobs; type more of the id")),
    }
}

/// Deletes a finished job's files.
pub fn remove(job: &Job) -> io::Result<()> {
    let _ = fs::remove_file(&job.log_path);
    let _ = fs::remove_file(stop_marker(&job.id));
    let _ = fs::remove_file(paused_marker(&job.id));
    fs::remove_file(job_path(&job.id))
}
