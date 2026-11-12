//! `ptrack run`: starts a command, owns it until it ends, and keeps its job
//! file up to date. In `--detach` mode the same code runs in a background
//! copy of ptrack.

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::config;
use crate::job::{self, HEARTBEAT_MS, Job, State};
use crate::output::{self, NO_OUTPUT, Output};
use crate::platform::{self, INTERRUPTS};
use crate::util::{clean_line, fmt_cpu, fmt_duration, now_ms};

/// Default time a job gets to exit on its own after a stop request.
pub const DEFAULT_GRACE_MS: u64 = 5_000;

#[derive(Default)]
pub struct RunOpts {
    pub label: Option<String>,
    pub agent: Option<String>,
    pub session: Option<String>,
    pub prompt_id: Option<String>,
    /// Folder to run in (default: the current folder). Used by `rerun`.
    pub cwd: Option<String>,
    /// Id of the job this one repeats (`ptrack rerun`).
    pub rerun_of: Option<String>,
    pub project: Option<String>,
    pub tag: Option<String>,
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
        return detach(&opts);
    }
    supervise(opts)
}

/// What `start_detached` found once the background supervisor started.
pub enum Detached {
    /// The job file exists; the job is running (or already ended).
    Started(Box<Job>),
    /// The supervisor started but has not written the job file within 5 s.
    NotYetReported(String),
}

/// Starts the job in a background copy of ptrack and waits until it has
/// written its job file, so an immediate `ptrack status` finds it.
/// Used by `ptrack run -d`, `ptrack rerun -d`, the dashboard and MCP.
pub fn start_detached(opts: &RunOpts) -> Result<Detached, String> {
    job::ensure_dirs().map_err(|e| format!("cannot create data folder: {e}"))?;
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
        ("--prompt-id", &opts.prompt_id),
        ("--cwd", &opts.cwd),
        ("--rerun-of", &opts.rerun_of),
        ("--project", &opts.project),
        ("--tag", &opts.tag),
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

    let exe = std::env::current_exe().map_err(|e| format!("cannot find own executable: {e}"))?;
    platform::spawn_detached(&exe, &args)
        .map_err(|e| format!("could not start background supervisor: {e}"))?;
    let deadline = now_ms() + 5_000;
    loop {
        if let Ok(job) = Job::load(&id) {
            if job.state == State::Failed && job.pid.is_none() {
                return Err(format!(
                    "job {id} failed to start: {}",
                    job.error.unwrap_or_default()
                ));
            }
            return Ok(Detached::Started(Box::new(job)));
        }
        if now_ms() > deadline {
            return Ok(Detached::NotYetReported(id));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn detach(opts: &RunOpts) -> i32 {
    match start_detached(opts) {
        Ok(Detached::Started(job)) => {
            let id = &job.id;
            println!("job {id} started in background");
            println!("  log:   {}", job.log_path);
            println!("  check: ptrack status {id}   |   ptrack wait {id}");
            println!("  watch: ptrack tail {id} -f   |   ptrack ui --job {id}");
            0
        }
        Ok(Detached::NotYetReported(id)) => {
            println!("job {id} launched, but has not reported yet; check with: ptrack status {id}");
            0
        }
        Err(msg) if msg.contains("failed to start") => {
            println!("{msg}");
            127
        }
        Err(msg) => {
            eprintln!("ptrack: {msg}");
            1
        }
    }
}

/// Variables that agents set for the commands they run, and the agent name
/// each one means. Checked in order, after `PTRACK_AGENT` and `AI_AGENT`.
const AGENT_MARKERS: &[(&str, &str)] = &[
    ("CLAUDECODE", "claude-code"),
    ("CODEX_THREAD_ID", "codex"),
    ("CODEX_SANDBOX", "codex"),
    ("CODEX_CI", "codex"),
    ("CURSOR_AGENT", "cursor"),
    ("ANTIGRAVITY_AGENT", "antigravity"),
    ("GEMINI_CLI", "gemini-cli"),
];

/// Variables that carry the agent's session id, checked in order.
const SESSION_MARKERS: &[&str] = &[
    "PTRACK_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CODEX_THREAD_ID",
];

fn detect_agent(opts: &RunOpts) -> (Option<String>, Option<String>) {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let agent = opts
        .agent
        .clone()
        .or_else(|| env("PTRACK_AGENT"))
        .or_else(|| env("AI_AGENT"))
        .or_else(|| {
            AGENT_MARKERS
                .iter()
                .find(|(var, _)| env(var).is_some())
                .map(|(_, name)| name.to_string())
        });
    let session = opts
        .session
        .clone()
        .or_else(|| SESSION_MARKERS.iter().find_map(|var| env(var)));
    (agent, session)
}

/// Project and tag: the option, else the environment variable, else (for the
/// project only) the name worked out from the folder.
fn detect_project_and_tag(opts: &RunOpts, cwd: &str) -> (Option<String>, Option<String>) {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let project = opts
        .project
        .clone()
        .or_else(|| env("PTRACK_PROJECT"))
        .or_else(|| job::project_for(cwd));
    let tag = opts.tag.clone().or_else(|| env("PTRACK_TAG"));
    (project, tag)
}

fn supervise(opts: RunOpts) -> i32 {
    platform::install_interrupt_handler();
    let id = opts.job_id.clone().unwrap_or_else(job::fresh_id);
    let mut job = Job::new(id.clone(), opts.command.clone(), opts.shell);
    job.label = opts.label.clone();
    (job.agent, job.session) = detect_agent(&opts);
    job.prompt_id = opts.prompt_id.clone();
    job.prompt = opts.prompt_id.as_deref().and_then(job::prompt_preview);
    job.rerun_of = opts.rerun_of.clone();
    if let Some(dir) = &opts.cwd {
        job.cwd = dir.clone();
    }
    (job.project, job.tag) = detect_project_and_tag(&opts, &job.cwd);
    let echo = !opts.background && !opts.quiet;
    let speak = !opts.background;

    let log = match Output::open(std::path::Path::new(&job.log_path)) {
        Ok(out) => Arc::new(out),
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
    if let Some(dir) = &opts.cwd {
        cmd.current_dir(dir);
    }
    // So wrappers know they run under ptrack and need not repeat what it
    // already writes (heartbeats, header, footer).
    let heartbeat_s = config::heartbeat_seconds(&config::load());
    cmd.env("PTRACK_JOB_ID", &id)
        .env("PTRACK_LOG", &job.log_path)
        .env("PTRACK_HEARTBEAT_SECONDS", heartbeat_s.to_string());
    // `_lifeline` must stay alive until this function returns: on Unix,
    // dropping it tells the guard process that the supervisor is gone.
    // (Naming it `_` instead would drop it immediately.)
    let (mut cmd, _lifeline) = match platform::prepare(cmd) {
        Ok(prepared) => prepared,
        Err(e) => {
            eprintln!("ptrack: cannot prepare the command: {e}");
            return 1;
        }
    };
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("could not start '{}': {e}", opts.command[0]);
            log.note(&header(&job));
            log.note(&[format!("# [ptrack] {msg}")]);
            job.error = Some(msg.clone());
            job.last_line = Some(msg.clone());
            job.finish(State::Failed, None);
            log.note(&[footer(&job, None)]);
            let _ = job.save();
            if speak {
                eprintln!("[ptrack] job {id}: {msg}");
            }
            return 127;
        }
    };
    // Before attach: on Windows the child is still suspended, and on Unix
    // nothing reads its output yet, so the header always comes first.
    job.pid = Some(child.id());
    log.note(&header(&job));
    let tree = match platform::attach(&child, &id) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("ptrack: could not take control of the process: {e}");
            let _ = child.kill();
            return 1;
        }
    };
    if let Err(e) = job.save() {
        eprintln!("ptrack: cannot write job file: {e}");
    }
    if echo {
        eprintln!(
            "[ptrack] job {id} started · watch it live: ptrack tail {id} -f · in your browser: ptrack ui --job {id}"
        );
    }

    let readers = [
        child
            .stdout
            .take()
            .map(|s| pump(s, log.clone(), echo.then_some(Echo::Out))),
        child
            .stderr
            .take()
            .map(|s| pump(s, log.clone(), echo.then_some(Echo::Err))),
    ];

    let mut cancelled = false;
    let mut killed = false;
    let mut stop_deadline = 0u64;
    let mut next_heartbeat = now_ms() + HEARTBEAT_MS;
    let mut cpu = platform::CpuMeter::default();
    let silence_every_ms = heartbeat_s.saturating_mul(1000);
    let mut last_note_ms = job.started_ms;
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
            job.last_line = log.last_line();
            job.last_output_ms = log.last_output_ms();
            job.cpu_ms = cpu.sample(&tree).or(job.cpu_ms);
            let _ = job.save();
            next_heartbeat = now + HEARTBEAT_MS;
        }
        if silence_every_ms > 0 {
            // Silent for the whole interval: since the last output and since
            // the last line ptrack wrote about it.
            let silent_since = log.last_output_ms().unwrap_or(job.started_ms);
            if now.saturating_sub(silent_since.max(last_note_ms)) >= silence_every_ms {
                job.cpu_ms = cpu.sample(&tree).or(job.cpu_ms);
                log.note(&[silence_line(&job, now, silent_since)]);
                last_note_ms = now;
            }
        }
        thread::sleep(Duration::from_millis(100));
    };

    // Output may still be buffered in the pipes. Background processes left
    // behind can keep a pipe open forever, so do not wait for them long.
    let deadline = now_ms() + 2_000;
    while readers.iter().flatten().any(|h| !h.is_finished()) && now_ms() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    // While the tree still exists: on Windows this is the exact final total.
    job.cpu_ms = cpu.sample(&tree).or(job.cpu_ms);
    job.last_output_ms = log.last_output_ms();

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
    let lines = log.line_count();
    job.last_line = match lines {
        None => Some(NO_OUTPUT.to_string()),
        Some(_) => log.last_line(),
    };
    job.finish(state, code);
    log.note(&[footer(&job, Some(lines))]);
    let _ = std::fs::remove_file(job::stop_marker(&id));
    let _ = std::fs::remove_file(job::paused_marker(&id));
    if let Err(e) = job.save() {
        eprintln!("ptrack: cannot write final job state: {e}");
    }
    crate::notify::job_finished(&job);

    if speak {
        eprintln!("{}", summary(&job));
        if opts.quiet && state != State::Passed {
            if let Ok(lines) = output::last_output_lines(std::path::Path::new(&job.log_path), 20) {
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

/// The lines that open every log, so a log is never empty and shows where a
/// redirected output went (the command line says `> file`).
fn header(job: &Job) -> Vec<String> {
    let mut lines = vec![match &job.label {
        Some(l) => format!("# ptrack {} {l}", job.id),
        None => format!("# ptrack {}", job.id),
    }];
    if job.shell {
        lines.extend(output::command_lines(&job.command.join(" ")));
        lines.push("# shell: yes".into());
    } else {
        lines.extend(output::command_lines(&job.command_line()));
    }
    lines.push(format!("# cwd: {}", job.cwd));
    if let Some(pid) = job.pid {
        lines.push(format!("# pid: {pid}"));
    }
    lines.push(format!("# started: {}", job.started_at));
    lines
}

/// The line that closes every log, e.g.
/// `# ended: 2026-11-12T10:00:00Z  exit=0  duration=00:04  output=3 lines  state=passed`.
/// `lines` is `None` when the command never started.
fn footer(job: &Job, lines: Option<Option<u64>>) -> String {
    let exit = job
        .exit_code
        .map_or_else(|| "?".to_string(), |c| c.to_string());
    let output = match lines {
        Some(count) => output::describe_lines(count),
        None => "none".into(),
    };
    format!(
        "# ended: {}  exit={exit}  duration={}  output={output}  state={}",
        job.ended_at.as_deref().unwrap_or("?"),
        output::clock_mm_ss(job.duration_ms()),
        job.state.as_str()
    )
}

/// What ptrack writes into the log while the command is silent, e.g.
/// `# [ptrack] 00:05:30 running, no output for 2m00s, tree cpu=41s`.
fn silence_line(job: &Job, now: u64, silent_since: u64) -> String {
    let doing = if job::paused_marker(&job.id).exists() {
        "paused"
    } else {
        "running"
    };
    let cpu = job
        .cpu_ms
        .map(|ms| format!(", tree cpu={}", fmt_cpu(ms)))
        .unwrap_or_default();
    format!(
        "# [ptrack] {} {doing}, no output for {}{cpu}",
        output::clock_hh_mm_ss(now.saturating_sub(job.started_ms)),
        fmt_duration(now.saturating_sub(silent_since)),
    )
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
    log: Arc<Output>,
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
            log.child_wrote(chunk, newest);
        }
    })
}

fn write_ignoring_errors(out: &mut dyn Write, data: &[u8]) {
    let _ = out.write_all(data);
    let _ = out.flush();
}
