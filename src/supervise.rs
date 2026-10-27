//! `ptrack run`: starts a command, owns it until it ends, and keeps its job
//! file up to date. In `--detach` mode the same code runs in a background
//! copy of ptrack.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::job::{self, HEARTBEAT_MS, Job, State};
use crate::platform::{self, INTERRUPTS};
use crate::util::{clean_line, fmt_duration, last_lines, now_ms};

/// Default time a job gets to exit on its own after a stop request.
pub const DEFAULT_GRACE_MS: u64 = 5_000;

#[derive(Default)]
pub struct RunOpts {
    pub label: Option<String>,
    pub agent: Option<String>,
    pub session: Option<String>,
    pub shell: bool,
    pub detach: bool,
    pub quiet: bool,
    /// Internal: this process is the detached supervisor.
    pub background: bool,
    /// Internal: id chosen by the process that detached us.
    pub job_id: Option<String>,
    pub command: Vec<String>,
}

pub fn run(opts: RunOpts) -> i32 {
    if let Err(e) = job::ensure_dirs() {
        eprintln!(
            "ptrack: cannot create data folder {}: {e}",
            job::home().display()
        );
        return 1;
    }
    if opts.detach && !opts.background {
        return detach(opts);
    }
    supervise(opts)
}

fn detach(opts: RunOpts) -> i32 {
    let id = job::fresh_id();
    let mut args: Vec<OsString> = vec![
        "run".into(),
        "--background".into(),
        "--job-id".into(),
        id.clone().into(),
    ];
    for (flag, value) in [
        ("--label", &opts.label),
        ("--agent", &opts.agent),
        ("--session", &opts.session),
    ] {
        if let Some(v) = value {
            args.push(flag.into());
            args.push(v.into());
        }
    }
    if opts.shell {
        args.push("--shell".into());
    }
    args.push("--".into());
    args.extend(opts.command.iter().map(OsString::from));

    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("ptrack: cannot find own executable: {e}");
            return 1;
        }
    };
    if let Err(e) = platform::spawn_detached(&exe, &args) {
        eprintln!("ptrack: could not start background supervisor: {e}");
        return 1;
    }
    // Wait until the supervisor has written the job file, so an immediate
    // `ptrack status` finds it.
    let deadline = now_ms() + 5_000;
    loop {
        if let Ok(job) = Job::load(&id) {
            if job.state == State::Failed && job.pid.is_none() {
                println!(
                    "job {id} failed to start: {}",
                    job.error.unwrap_or_default()
                );
                return 127;
            }
            println!("job {id} started in background");
            println!("  log:   {}", job.log_path);
            println!("  check: ptrack status {id}   |   ptrack wait {id}");
            return 0;
        }
        if now_ms() > deadline {
            println!("job {id} launched, but has not reported yet; check with: ptrack status {id}");
            return 0;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn detect_agent(opts: &RunOpts) -> (Option<String>, Option<String>) {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let agent = opts
        .agent
        .clone()
        .or_else(|| env("PTRACK_AGENT"))
        .or_else(|| env("AI_AGENT"))
        .or_else(|| env("CLAUDECODE").map(|_| "claude-code".to_string()));
    let session = opts
        .session
        .clone()
        .or_else(|| env("PTRACK_SESSION"))
        .or_else(|| env("CLAUDE_CODE_SESSION_ID"));
    (agent, session)
}

fn supervise(opts: RunOpts) -> i32 {
    platform::install_interrupt_handler();
    let id = opts.job_id.clone().unwrap_or_else(job::fresh_id);
    let mut job = Job::new(id.clone(), opts.command.clone(), opts.shell);
    job.label = opts.label.clone();
    (job.agent, job.session) = detect_agent(&opts);
    let echo = !opts.background && !opts.quiet;
    let speak = !opts.background;

    let log = match OpenOptions::new()
        .create(true)
        .append(true)
        .open(&job.log_path)
    {
        Ok(f) => Arc::new(Mutex::new(f)),
        Err(e) => {
            eprintln!("ptrack: cannot open log file {}: {e}", job.log_path);
            return 1;
        }
    };

    let mut cmd = if opts.shell {
        platform::shell_command(&opts.command.join(" "))
    } else {
        let mut c = Command::new(platform::resolve_program(&opts.command[0]));
        c.args(&opts.command[1..]);
        c
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    platform::prepare(&mut cmd);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("could not start '{}': {e}", opts.command[0]);
            let _ = writeln!(log.lock().unwrap(), "[ptrack] {msg}");
            job.error = Some(msg.clone());
            job.last_line = Some(msg.clone());
            job.finish(State::Failed, None);
            let _ = job.save();
            if speak {
                eprintln!("[ptrack] job {id}: {msg}");
            }
            return 127;
        }
    };
    let tree = match platform::attach(&child, &id) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("ptrack: could not take control of the process: {e}");
            let _ = child.kill();
            return 1;
        }
    };
    job.pid = Some(child.id());
    if let Err(e) = job.save() {
        eprintln!("ptrack: cannot write job file: {e}");
    }
    if echo {
        eprintln!("[ptrack] job {id} started · log: {}", job.log_path);
    }

    let last = Arc::new(Mutex::new(None::<String>));
    let readers = [
        child
            .stdout
            .take()
            .map(|s| pump(s, log.clone(), last.clone(), echo.then_some(Echo::Out))),
        child
            .stderr
            .take()
            .map(|s| pump(s, log.clone(), last.clone(), echo.then_some(Echo::Err))),
    ];

    let mut cancelled = false;
    let mut killed = false;
    let mut stop_deadline = 0u64;
    let mut next_heartbeat = now_ms() + HEARTBEAT_MS;
    let status: Option<ExitStatus> = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) => {}
            Err(_) => break None,
        }
        let now = now_ms();
        let interrupts = INTERRUPTS.load(Ordering::SeqCst);
        if !cancelled {
            let grace = if interrupts > 0 {
                Some(DEFAULT_GRACE_MS)
            } else {
                read_stop_request(&id)
            };
            if let Some(grace) = grace {
                cancelled = true;
                stop_deadline = now + grace;
                if speak {
                    eprintln!("[ptrack] stopping job {id}…");
                }
                if grace == 0 {
                    platform::kill(&tree);
                    killed = true;
                } else {
                    platform::interrupt(&tree);
                }
            }
        } else if !killed && (now >= stop_deadline || interrupts >= 2) {
            platform::kill(&tree);
            killed = true;
        }
        if now >= next_heartbeat {
            job.heartbeat_ms = now;
            job.last_line = last.lock().unwrap().clone();
            let _ = job.save();
            next_heartbeat = now + HEARTBEAT_MS;
        }
        thread::sleep(Duration::from_millis(100));
    };

    // Output may still be buffered in the pipes. Background processes left
    // behind can keep a pipe open forever, so do not wait for them long.
    let deadline = now_ms() + 2_000;
    while readers.iter().flatten().any(|h| !h.is_finished()) && now_ms() < deadline {
        thread::sleep(Duration::from_millis(20));
    }

    if cancelled {
        platform::kill(&tree);
    } else {
        platform::release(&tree);
    }

    let code = status.as_ref().and_then(exit_code);
    let state = if cancelled {
        State::Cancelled
    } else if code == Some(0) {
        State::Passed
    } else {
        State::Failed
    };
    job.last_line = last.lock().unwrap().clone();
    job.finish(state, code);
    let _ = std::fs::remove_file(job::stop_marker(&id));
    let _ = std::fs::remove_file(job::paused_marker(&id));
    if let Err(e) = job.save() {
        eprintln!("ptrack: cannot write final job state: {e}");
    }

    if speak {
        eprintln!("{}", summary(&job));
        if opts.quiet && state != State::Passed {
            if let Ok(lines) = last_lines(std::path::Path::new(&job.log_path), 20) {
                for l in lines {
                    eprintln!("  {l}");
                }
            }
        }
    }
    match (state, code) {
        (State::Passed, _) => 0,
        (_, Some(c)) if c != 0 => c,
        _ => 1,
    }
}

/// One-line result, e.g. `[ptrack] job k3f9a2 failed after 1m02s (exit 1)`.
pub fn summary(job: &Job) -> String {
    let state = job.effective_state();
    let exit = job
        .exit_code
        .map(|c| format!(" (exit {c})"))
        .unwrap_or_default();
    format!(
        "[ptrack] job {} {} after {}{exit} · {}",
        job.id,
        state.as_str(),
        fmt_duration(job.duration_ms()),
        job.title()
    )
}

fn read_stop_request(id: &str) -> Option<u64> {
    let text = std::fs::read_to_string(job::stop_marker(id)).ok()?;
    Some(text.trim().parse().unwrap_or(DEFAULT_GRACE_MS))
}

#[cfg(unix)]
fn exit_code(s: &ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    s.code().or_else(|| s.signal().map(|sig| 128 + sig))
}

#[cfg(windows)]
fn exit_code(s: &ExitStatus) -> Option<i32> {
    s.code()
}

#[derive(Clone, Copy)]
enum Echo {
    Out,
    Err,
}

/// Copies one output stream into the log (and optionally the terminal), and
/// keeps track of the last meaningful line.
fn pump<R: Read + Send + 'static>(
    mut src: R,
    log: Arc<Mutex<File>>,
    last: Arc<Mutex<Option<String>>>,
    echo: Option<Echo>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        let mut pending: Vec<u8> = Vec::new();
        loop {
            let n = match src.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let chunk = &buf[..n];
            let _ = log.lock().unwrap().write_all(chunk);
            match echo {
                Some(Echo::Out) => write_ignoring_errors(&mut io::stdout(), chunk),
                Some(Echo::Err) => write_ignoring_errors(&mut io::stderr(), chunk),
                None => {}
            }
            pending.extend_from_slice(chunk);
            let mut newest = None;
            while let Some(pos) = pending.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = pending.drain(..=pos).collect();
                if let Some(text) = clean_line(&line) {
                    newest = Some(text);
                }
            }
            if pending.len() > 4096 {
                pending.drain(..pending.len() - 4096);
            }
            if let Some(text) = clean_line(&pending) {
                newest = Some(text);
            }
            if newest.is_some() {
                *last.lock().unwrap() = newest;
            }
        }
    })
}

fn write_ignoring_errors(out: &mut dyn Write, data: &[u8]) {
    let _ = out.write_all(data);
    let _ = out.flush();
}
