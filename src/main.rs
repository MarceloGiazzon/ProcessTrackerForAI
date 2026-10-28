//! ptrack: see and control the long-running processes your AI agents start.

mod commands;
mod hook;
mod job;
mod mcp;
mod platform;
mod supervise;
mod util;
mod web;

use std::collections::VecDeque;
use std::process::ExitCode;

use supervise::RunOpts;

const HELP: &str = "\
ptrack - see and control the long-running processes your AI agents start

USAGE
  ptrack run [options] -- <command> [args...]   run a command as a tracked job
  ptrack list [--all] [--here] [--json]         what is running, and what ran recently
  ptrack status [ID] [--json]                   details of one job (default: newest)
  ptrack tail [ID] [-n LINES] [-f]              show the job's output (-f: follow)
  ptrack wait [ID] [--timeout SECS] [-n LINES]  block until the job ends; exits with its code
  ptrack stop ID [--grace SECS] [--force]       stop the job and everything it started
  ptrack pause ID | ptrack resume ID            freeze / unfreeze the whole job
  ptrack prune [--days N] [--all]               delete finished jobs (default: older than 7 days)
  ptrack ui [--port N] [--no-open]              live dashboard in your browser (this computer only)
  ptrack mcp [--allow-control]                  MCP server for AI agents (stdio); read-only unless
                                                --allow-control (adds stop / pause / resume)
  ptrack hook install [--project] [--all] [--no-prompts]
                                                make Claude Code run long commands through ptrack
  ptrack hook uninstall [--project]             remove that hook again

RUN OPTIONS
  -l, --label TEXT    a human-readable name for the job
  -d, --detach        run in the background and return immediately (prints the job id)
  -q, --quiet         don't echo output; print one summary line (+ last lines on failure)
  -s, --shell         run the command line through the system shell (cmd.exe or sh)
      --agent NAME    who started it (default: detected from AI_AGENT / CLAUDECODE)
      --session ID    agent session (default: detected from CLAUDE_CODE_SESSION_ID)

ID can be a unique prefix of a job id, or 'last'.
Data folder: set PTRACK_HOME to override the default per-user location.
";

fn main() -> ExitCode {
    let args: VecDeque<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let code = match dispatch(args) {
        Ok(code) => code,
        Err(msg) => {
            eprintln!("ptrack: {msg}");
            2
        }
    };
    ExitCode::from(code.clamp(0, 255) as u8)
}

fn dispatch(mut args: VecDeque<String>) -> Result<i32, String> {
    let Some(cmd) = args.pop_front() else {
        print!("{HELP}");
        return Ok(0);
    };
    match cmd.as_str() {
        "run" => Ok(supervise::run(parse_run(args)?)),
        "list" | "ls" => {
            let mut p = Parser::new(args);
            let (mut all, mut here, mut json) = (false, false, false);
            while let Some(a) = p.next_flag()? {
                match a.as_str() {
                    "--all" | "-a" => all = true,
                    "--here" => here = true,
                    "--json" => json = true,
                    _ => return Err(unknown(&a)),
                }
            }
            p.no_more()?;
            commands::list(all, here, json)
        }
        "status" => {
            let mut p = Parser::new(args);
            let mut json = false;
            while let Some(a) = p.next_flag()? {
                match a.as_str() {
                    "--json" => json = true,
                    _ => return Err(unknown(&a)),
                }
            }
            let id = p.positional();
            p.no_more()?;
            commands::status(id.as_deref(), json)
        }
        "tail" | "logs" => {
            let mut p = Parser::new(args);
            let (mut lines, mut follow) = (40usize, false);
            while let Some(a) = p.next_flag()? {
                match a.as_str() {
                    "-n" | "--lines" => lines = p.number(&a)?,
                    "-f" | "--follow" => follow = true,
                    _ => return Err(unknown(&a)),
                }
            }
            let id = p.positional();
            p.no_more()?;
            commands::tail(id.as_deref(), lines, follow)
        }
        "wait" => {
            let mut p = Parser::new(args);
            let (mut timeout, mut lines) = (None, None);
            while let Some(a) = p.next_flag()? {
                match a.as_str() {
                    "--timeout" | "-t" => timeout = Some(p.number::<u64>(&a)?),
                    "-n" | "--lines" => lines = Some(p.number(&a)?),
                    _ => return Err(unknown(&a)),
                }
            }
            let id = p.positional();
            p.no_more()?;
            commands::wait(id.as_deref(), timeout, lines)
        }
        "stop" | "kill" => {
            let mut p = Parser::new(args);
            let mut grace = supervise::DEFAULT_GRACE_MS / 1000;
            while let Some(a) = p.next_flag()? {
                match a.as_str() {
                    "--grace" => grace = p.number(&a)?,
                    "--force" | "-f" => grace = 0,
                    _ => return Err(unknown(&a)),
                }
            }
            let id = p.required_id("stop")?;
            p.no_more()?;
            commands::stop(&id, grace * 1000)
        }
        "pause" | "resume" => {
            let mut p = Parser::new(args);
            if let Some(a) = p.next_flag()? {
                return Err(unknown(&a));
            }
            let id = p.required_id(&cmd)?;
            p.no_more()?;
            if cmd == "pause" {
                commands::pause(&id)
            } else {
                commands::resume(&id)
            }
        }
        "prune" => {
            let mut p = Parser::new(args);
            let (mut days, mut all) = (7u64, false);
            while let Some(a) = p.next_flag()? {
                match a.as_str() {
                    "--days" => days = p.number(&a)?,
                    "--all" => all = true,
                    _ => return Err(unknown(&a)),
                }
            }
            p.no_more()?;
            commands::prune(if all { 0 } else { days })
        }
        "ui" | "dashboard" => {
            let mut p = Parser::new(args);
            let (mut port, mut open) = (None, true);
            while let Some(a) = p.next_flag()? {
                match a.as_str() {
                    "--port" | "-p" => port = Some(p.number(&a)?),
                    "--no-open" => open = false,
                    _ => return Err(unknown(&a)),
                }
            }
            p.no_more()?;
            Ok(web::serve(port, open))
        }
        "mcp" => {
            let mut p = Parser::new(args);
            let mut allow_control = false;
            while let Some(a) = p.next_flag()? {
                match a.as_str() {
                    "--allow-control" => allow_control = true,
                    _ => return Err(unknown(&a)),
                }
            }
            p.no_more()?;
            Ok(mcp::serve(allow_control))
        }
        "hook" => {
            let what = args.pop_front().unwrap_or_default();
            let mut p = Parser::new(args);
            let (mut project, mut all, mut prompts) = (false, false, true);
            while let Some(a) = p.next_flag()? {
                match a.as_str() {
                    "--project" => project = true,
                    "--all" => all = true,
                    "--no-prompts" => prompts = false,
                    _ => return Err(unknown(&a)),
                }
            }
            p.no_more()?;
            match what.as_str() {
                // Called by Claude Code itself; must never fail loudly.
                "claude-code" => Ok(hook::run_claude_code(all)),
                "install" => hook::install(hook::InstallOpts {
                    project,
                    wrap_all: all,
                    record_prompts: prompts,
                }),
                "uninstall" => hook::uninstall(project),
                _ => Err("usage: ptrack hook install|uninstall [--project]".into()),
            }
        }
        "help" | "--help" | "-h" => {
            print!("{HELP}");
            Ok(0)
        }
        "version" | "--version" | "-V" => {
            println!("ptrack {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        other => Err(format!("unknown command '{other}' (try: ptrack help)")),
    }
}

fn parse_run(args: VecDeque<String>) -> Result<RunOpts, String> {
    let mut p = Parser::new(args);
    p.command_follows = true;
    let mut opts = RunOpts::default();
    while let Some(a) = p.next_flag()? {
        match a.as_str() {
            "-l" | "--label" => opts.label = Some(p.value(&a)?),
            "-d" | "--detach" => opts.detach = true,
            "-q" | "--quiet" => opts.quiet = true,
            "-s" | "--shell" => opts.shell = true,
            "--agent" => opts.agent = Some(p.value(&a)?),
            "--session" => opts.session = Some(p.value(&a)?),
            "--prompt-id" => opts.prompt_id = Some(p.value(&a)?),
            "--background" => opts.background = true,
            "--job-id" => opts.job_id = Some(p.value(&a)?),
            _ => return Err(format!("{} (put the command after --)", unknown(&a))),
        }
    }
    opts.command = p.rest();
    if opts.command.is_empty() || opts.command[0].is_empty() {
        return Err("nothing to run. Usage: ptrack run [options] -- <command> [args...]".into());
    }
    Ok(opts)
}

fn unknown(flag: &str) -> String {
    format!("unknown option '{flag}'")
}

/// Minimal argument reader: flags first, then positionals. `--` ends flags.
struct Parser {
    args: VecDeque<String>,
    positionals: VecDeque<String>,
    ended: bool,
    /// For `run`: the first positional starts the command, ending flags.
    command_follows: bool,
}

impl Parser {
    fn new(args: VecDeque<String>) -> Parser {
        Parser {
            args,
            positionals: VecDeque::new(),
            ended: false,
            command_follows: false,
        }
    }

    /// Next flag. Positionals in between are kept for later, except for
    /// `run`, where the first positional ends flag parsing.
    fn next_flag(&mut self) -> Result<Option<String>, String> {
        if self.ended {
            return Ok(None);
        }
        while let Some(a) = self.args.pop_front() {
            if a == "--" {
                self.ended = true;
                return Ok(None);
            }
            if a.starts_with('-') && a.len() > 1 {
                return Ok(Some(a));
            }
            if self.command_follows {
                self.args.push_front(a);
                self.ended = true;
                return Ok(None);
            }
            self.positionals.push_back(a);
        }
        Ok(None)
    }

    fn value(&mut self, flag: &str) -> Result<String, String> {
        self.args
            .pop_front()
            .ok_or_else(|| format!("{flag} needs a value"))
    }

    fn number<T: std::str::FromStr>(&mut self, flag: &str) -> Result<T, String> {
        let v = self.value(flag)?;
        v.parse()
            .map_err(|_| format!("{flag} needs a number, got '{v}'"))
    }

    fn positional(&mut self) -> Option<String> {
        self.positionals.pop_front().or_else(|| {
            if self.ended {
                self.args.pop_front()
            } else {
                None
            }
        })
    }

    fn required_id(&mut self, cmd: &str) -> Result<String, String> {
        self.positional()
            .ok_or_else(|| format!("{cmd} needs a job id (see: ptrack list)"))
    }

    /// Everything left, for `run`: positionals seen so far, then the rest.
    fn rest(&mut self) -> Vec<String> {
        let mut out: Vec<String> = self.positionals.drain(..).collect();
        out.extend(self.args.drain(..));
        out
    }

    fn no_more(&self) -> Result<(), String> {
        match self.positionals.front().or(self.args.front()) {
            Some(extra) => Err(format!("unexpected argument '{extra}'")),
            None => Ok(()),
        }
    }
}
