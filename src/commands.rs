//! Read-side and control commands: list, status, tail, wait, stop, pause,
//! resume, prune. None of them write a job file; they only read it, or leave
//! a marker file for the job's supervisor.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::thread;
use std::time::Duration;

use crate::job::{self, Job, State};
use crate::platform;
use crate::supervise::summary;
use crate::util::{fmt_duration, last_lines, now_ms, truncate};

type CmdResult = Result<i32, String>;

/// Finished jobs older than this are hidden from `list` unless `--all`.
const RECENT_MS: u64 = 24 * 60 * 60 * 1000;
const RECENT_MAX: usize = 15;

fn to_json(job: &Job) -> serde_json::Value {
    let mut shown = job.clone();
    shown.state = job.effective_state();
    let mut v = serde_json::to_value(&shown).unwrap_or_default();
    v["duration_ms"] = job.duration_ms().into();
    v
}

pub fn list(all: bool, here: bool, json: bool) -> CmdResult {
    let mut jobs = job::list_all();
    if here {
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        jobs.retain(|j| j.cwd.starts_with(&cwd));
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
    let state = j.effective_state();
    if state == State::Lost {
        let hint = if cfg!(unix) {
            j.pid
                .map(|p| {
                    format!(
                        " If its processes survived, they are in process group {p}: kill -- -{p}"
                    )
                })
                .unwrap_or_default()
        } else {
            String::new()
        };
        println!(
            "job {} is lost: its supervisor is gone, so there is nothing ptrack still controls.{hint}",
            j.id
        );
        return Ok(1);
    }
    if !state.is_active() {
        println!("job {} already {}", j.id, state.as_str());
        return Ok(0);
    }
    std::fs::write(job::stop_marker(&j.id), grace_ms.to_string())
        .map_err(|e| format!("cannot write stop request: {e}"))?;
    if grace_ms > 0 {
        println!(
            "stopping job {} (up to {} for it to exit on its own)…",
            j.id,
            fmt_duration(grace_ms)
        );
    }

    // The supervisor checks for the request ten times a second.
    if let Some(done) = wait_until_inactive(&j.id, grace_ms + 5_000) {
        println!("{}", summary(&done));
        return Ok(0);
    }
    // Supervisor did not react: stop the tree ourselves.
    let pid = j.pid.unwrap_or(0);
    platform::kill_external(&j.id, pid).map_err(|e| format!("could not stop job {}: {e}", j.id))?;
    match wait_until_inactive(&j.id, 3_000) {
        Some(done) => {
            println!("{}", summary(&done));
            Ok(0)
        }
        None => {
            println!(
                "job {} was force-stopped, but its supervisor has not recorded the end yet",
                j.id
            );
            Ok(1)
        }
    }
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
    match j.effective_state() {
        State::Running => {}
        State::Paused => {
            println!("job {} is already paused", j.id);
            return Ok(0);
        }
        s => return Err(format!("job {} is {}, not running", j.id, s.as_str())),
    }
    let pid = j.pid.ok_or("job has no process yet")?;
    platform::suspend_external(&j.id, pid)
        .map_err(|e| format!("could not pause job {}: {e}", j.id))?;
    let _ = std::fs::write(job::paused_marker(&j.id), now_ms().to_string());
    println!("paused job {} · resume with: ptrack resume {}", j.id, j.id);
    println!("note: while paused, network connections and timers inside the job may time out.");
    Ok(0)
}

pub fn resume(id: &str) -> CmdResult {
    let j = job::resolve(Some(id))?;
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
    println!("resumed job {}", j.id);
    Ok(0)
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
    println!("removed {removed} finished job(s)");
    Ok(0)
}
