//! Read-side and control commands: list, status, tail, wait, stop, pause,
//! resume, prune. None of them write a job file; they only read it, or leave
//! a marker file for the job's supervisor.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::thread;
use std::time::Duration;

use crate::job::{self, Job, State};
use crate::platform;
use crate::supervise::{self, RunOpts, summary};
use crate::util::{fmt_duration, last_lines, now_ms, truncate};

type CmdResult = Result<i32, String>;

/// Finished jobs older than this are hidden from `list` unless `--all`.
const RECENT_MS: u64 = 24 * 60 * 60 * 1000;
const RECENT_MAX: usize = 15;

/// A job as JSON, with the state as it is right now and its duration.
pub fn to_json(job: &Job) -> serde_json::Value {
    let mut shown = job.clone();
    shown.state = job.effective_state();
    let mut v = serde_json::to_value(&shown).unwrap_or_default();
    v["duration_ms"] = job.duration_ms().into();
    v
}

/// Jobs worth showing: all active ones plus recent finished ones (or all
/// with `all`), optionally only those started under `cwd_prefix`.
/// Also returns how many jobs matched before hiding old ones.
pub fn visible_jobs(all: bool, cwd_prefix: Option<&str>) -> (Vec<Job>, usize) {
    let mut jobs = job::list_all();
    if let Some(prefix) = cwd_prefix {
        jobs.retain(|j| j.cwd.starts_with(prefix));
    }
    let total = jobs.len();
    if !all {
        let now = now_ms();
        let mut recent = 0;
        jobs.retain(|j| {
            if j.effective_state().is_active() {
                return true;
            }
            let ended = j.ended_ms.unwrap_or(j.heartbeat_ms);
            recent += 1;
            recent <= RECENT_MAX && now.saturating_sub(ended) < RECENT_MS
        });
    }
    (jobs, total)
}

pub fn list(all: bool, here: bool, json: bool) -> CmdResult {
    let cwd = here.then(|| {
        std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    });
    let (jobs, total) = visible_jobs(all, cwd.as_deref());

    if json {
        let out: Vec<_> = jobs.iter().map(to_json).collect();
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
        return Ok(0);
    }
    if jobs.is_empty() {
        if total == 0 {
            println!("No jobs yet. Start one with: ptrack run -- <command>");
        } else {
            println!("Nothing running or recent. {total} older job(s): ptrack list --all");
        }
        return Ok(0);
    }

    let now = now_ms();
    println!(
        "{:<7} {:<10} {:>7} {:>9}  {:<12} {:<34} LAST LINE",
        "ID", "STATE", "TIME", "STARTED", "AGENT", "WHAT"
    );
    for j in &jobs {
        let agent = j
            .agent
            .as_deref()
            .map(|a| a.split('_').next().unwrap_or(a))
            .unwrap_or("-");
        println!(
            "{:<7} {:<10} {:>7} {:>9}  {:<12} {:<34} {}",
            j.id,
            j.effective_state().as_str(),
            fmt_duration(j.duration_ms()),
            format!("{} ago", fmt_duration(now.saturating_sub(j.started_ms))),
            truncate(agent, 12),
            truncate(&j.title(), 34),
            truncate(j.last_line.as_deref().unwrap_or(""), 60),
        );
    }
    if jobs.len() < total {
        println!(
            "({} older job(s) hidden: ptrack list --all)",
            total - jobs.len()
        );
    }
    Ok(0)
}

pub fn status(id: Option<&str>, json: bool) -> CmdResult {
    let j = job::resolve(id)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&to_json(&j)).unwrap_or_default()
        );
        return Ok(0);
    }
    let state = j.effective_state();
    let now = now_ms();
    let row = |k: &str, v: &str| println!("{k:<9} {v}");
    row("job", &j.id);
    let state_text = match (state, j.pid) {
        (s, Some(pid)) if s.is_active() => format!("{} (pid {pid})", s.as_str()),
        (State::Lost, _) => format!(
            "lost (no heartbeat for {}; its supervisor is gone)",
            fmt_duration(now.saturating_sub(j.heartbeat_ms))
        ),
        (s, _) => s.as_str().to_string(),
    };
    row("state", &state_text);
    if let Some(l) = &j.label {
        row("label", l);
    }
    row("command", &j.command_line());
    row("cwd", &j.cwd);
    if let Some(a) = &j.agent {
        row("agent", a);
    }
    if let Some(s) = &j.session {
        row("session", s);
    }
    if let Some(p) = &j.prompt {
        row("prompt", p);
    }
    if let Some(r) = &j.rerun_of {
        row("rerun of", r);
    }
    row(
        "started",
        &format!(
            "{} ({} ago)",
            j.started_at,
            fmt_duration(now.saturating_sub(j.started_ms))
        ),
    );
    if let Some(e) = &j.ended_at {
        row("ended", e);
    }
    row("duration", &fmt_duration(j.duration_ms()));
    if let Some(c) = j.exit_code {
        row("exit", &c.to_string());
    }
    if let Some(e) = &j.error {
        row("error", e);
    }
    row("log", &j.log_path);
    if let Some(l) = &j.last_line {
        row("last", l);
    }
    Ok(0)
}

pub fn tail(id: Option<&str>, lines: usize, follow: bool) -> CmdResult {
    let j = job::resolve(id)?;
    let path = Path::new(&j.log_path);
    let mut out = std::io::stdout();
    match last_lines(path, lines) {
        Ok(ls) => {
            for l in ls {
                let _ = writeln!(out, "{l}");
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("cannot read {}: {e}", j.log_path)),
    }
    if !follow {
        return Ok(0);
    }
    let mut pos = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    loop {
        let active = job::Job::load(&j.id)
            .map(|j| j.effective_state().is_active())
            .unwrap_or(false);
        if let Ok(mut f) = std::fs::File::open(path) {
            if f.seek(SeekFrom::Start(pos)).is_ok() {
                let mut buf = Vec::new();
                if f.read_to_end(&mut buf).is_ok() && !buf.is_empty() {
                    pos += buf.len() as u64;
                    let _ = out.write_all(&buf);
                    let _ = out.flush();
                }
            }
        }
        if !active {
            break;
        }
        thread::sleep(Duration::from_millis(300));
    }
    if let Ok(done) = Job::load(&j.id) {
        eprintln!("{}", summary(&done));
    }
    Ok(0)
}

pub fn wait(id: Option<&str>, timeout_s: Option<u64>, lines: Option<usize>) -> CmdResult {
    let start = job::resolve(id)?;
    let deadline = timeout_s.map(|t| now_ms() + t * 1000);
    let done = loop {
        let j = Job::load(&start.id).map_err(|e| format!("job {} vanished: {e}", start.id))?;
        if !j.effective_state().is_active() {
            break j;
        }
        if deadline.is_some_and(|d| now_ms() >= d) {
            println!(
                "[ptrack] job {} still {} after {} · {}",
                j.id,
                j.effective_state().as_str(),
                fmt_duration(j.duration_ms()),
                j.last_line.as_deref().unwrap_or("")
            );
            return Ok(124);
        }
        thread::sleep(Duration::from_millis(500));
    };
    let state = done.effective_state();
    println!("{}", summary(&done));
    let n = lines.unwrap_or(if state == State::Passed { 0 } else { 20 });
    if let Ok(ls) = last_lines(Path::new(&done.log_path), n) {
        for l in ls {
            println!("  {l}");
        }
    }
    Ok(match (state, done.exit_code) {
        (State::Passed, _) => 0,
        (_, Some(c)) if c != 0 => c,
        _ => 1,
    })
}

pub fn stop(id: &str, grace_ms: u64) -> CmdResult {
    let j = job::resolve(Some(id))?;
    if j.effective_state().is_active() && grace_ms > 0 {
        println!(
            "stopping job {} (up to {} for it to exit on its own)…",
            j.id,
            fmt_duration(grace_ms)
        );
    }
    match stop_job(&j, grace_ms)? {
        Stopped::Already(done) => println!(
            "job {} already {}",
            done.id,
            done.effective_state().as_str()
        ),
        Stopped::Now(done) => println!("{}", summary(&done)),
    }
    Ok(0)
}

pub enum Stopped {
    Already(Job),
    Now(Job),
}

/// Asks the job's supervisor to stop it, waits for the recorded end, and
/// force-stops the tree directly if the supervisor does not react.
pub fn stop_job(j: &Job, grace_ms: u64) -> Result<Stopped, String> {
    let state = j.effective_state();
    if state == State::Lost {
        let hint = match j.pid {
            Some(p) if cfg!(unix) => {
                format!(" If its processes survived, they are in process group {p}: kill -- -{p}")
            }
            _ => String::new(),
        };
        return Err(format!(
            "job {} is lost: its supervisor is gone, so there is nothing ptrack still controls.{hint}",
            j.id
        ));
    }
    if !state.is_active() {
        return Ok(Stopped::Already(j.clone()));
    }
    std::fs::write(job::stop_marker(&j.id), grace_ms.to_string())
        .map_err(|e| format!("cannot write stop request: {e}"))?;

    // The supervisor checks for the request ten times a second.
    if let Some(done) = wait_until_inactive(&j.id, grace_ms + 5_000) {
        return Ok(Stopped::Now(done));
    }
    // Supervisor did not react: stop the tree ourselves.
    platform::kill_external(&j.id, j.pid.unwrap_or(0))
        .map_err(|e| format!("could not stop job {}: {e}", j.id))?;
    wait_until_inactive(&j.id, 3_000)
        .map(Stopped::Now)
        .ok_or_else(|| {
            format!(
                "job {} was force-stopped, but its supervisor has not recorded the end yet",
                j.id
            )
        })
}

fn wait_until_inactive(id: &str, within_ms: u64) -> Option<Job> {
    let deadline = now_ms() + within_ms;
    while now_ms() < deadline {
        if let Ok(j) = Job::load(id) {
            if !j.effective_state().is_active() {
                return Some(j);
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
    None
}

pub fn pause(id: &str) -> CmdResult {
    let j = job::resolve(Some(id))?;
    if !pause_job(&j)? {
        println!("job {} is already paused", j.id);
        return Ok(0);
    }
    println!("paused job {} · resume with: ptrack resume {}", j.id, j.id);
    println!("note: while paused, network connections and timers inside the job may time out.");
    Ok(0)
}

pub fn resume(id: &str) -> CmdResult {
    let j = job::resolve(Some(id))?;
    resume_job(&j)?;
    println!("resumed job {}", j.id);
    Ok(0)
}

/// Freezes every process of a running job. Returns false if it was already paused.
pub fn pause_job(j: &Job) -> Result<bool, String> {
    match j.effective_state() {
        State::Running => {}
        State::Paused => return Ok(false),
        s => return Err(format!("job {} is {}, not running", j.id, s.as_str())),
    }
    let pid = j.pid.ok_or("job has no process yet")?;
    platform::suspend_external(&j.id, pid)
        .map_err(|e| format!("could not pause job {}: {e}", j.id))?;
    let _ = std::fs::write(job::paused_marker(&j.id), now_ms().to_string());
    Ok(true)
}

pub fn resume_job(j: &Job) -> Result<(), String> {
    if j.effective_state() != State::Paused {
        return Err(format!(
            "job {} is {}, not paused",
            j.id,
            j.effective_state().as_str()
        ));
    }
    let pid = j.pid.ok_or("job has no process")?;
    platform::resume_external(&j.id, pid)
        .map_err(|e| format!("could not resume job {}: {e}", j.id))?;
    let _ = std::fs::remove_file(job::paused_marker(&j.id));
    Ok(())
}

pub fn prune(older_than_days: u64) -> CmdResult {
    let cutoff = now_ms().saturating_sub(older_than_days * 24 * 60 * 60 * 1000);
    let mut removed = 0;
    for j in job::list_all() {
        if j.effective_state().is_active() {
            continue;
        }
        if j.ended_ms.unwrap_or(j.heartbeat_ms) <= cutoff && job::remove(&j).is_ok() {
            removed += 1;
        }
    }
    // Prompt previews are only needed while jobs refer to them.
    if let Ok(entries) = std::fs::read_dir(job::prompts_dir()) {
        let cutoff = std::time::SystemTime::now()
            - Duration::from_millis(older_than_days * 24 * 60 * 60 * 1000);
        for e in entries.flatten() {
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| t <= cutoff);
            if old {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    println!("removed {removed} finished job(s)");
    Ok(0)
}

/// The options that run `j`'s command again, in the same folder, as a new
/// job linked to the old one. Refuses while `j` is still active.
pub fn rerun_opts(j: &Job) -> Result<RunOpts, String> {
    let state = j.effective_state();
    if state.is_active() {
        return Err(format!(
            "job {} is still {}; stop it first (ptrack stop {})",
            j.id,
            state.as_str(),
            j.id
        ));
    }
    Ok(RunOpts {
        label: j.label.clone(),
        shell: j.shell,
        command: j.command.clone(),
        cwd: Some(j.cwd.clone()),
        rerun_of: Some(j.id.clone()),
        ..RunOpts::default()
    })
}

pub fn rerun(id: &str, detach: bool, quiet: bool) -> CmdResult {
    let j = job::resolve(Some(id))?;
    let mut opts = rerun_opts(&j)?;
    opts.detach = detach;
    opts.quiet = quiet;
    Ok(supervise::run(opts))
}

/// Starts a rerun in the background; returns a one-line message with the new id.
/// Used by the dashboard and the MCP server.
pub fn rerun_detached(j: &Job) -> Result<String, String> {
    let opts = rerun_opts(j)?;
    match supervise::start_detached(&opts)? {
        supervise::Detached::Started(new) => {
            Ok(format!("started job {} (rerun of {})", new.id, j.id))
        }
        supervise::Detached::NotYetReported(new_id) => Ok(format!(
            "launched job {new_id} (rerun of {}); it has not reported yet",
            j.id
        )),
    }
}
