//! `ptrack mcp`: a Model Context Protocol server over stdio, so agents can
//! ask about jobs with one small tool call instead of reading logs.
//!
//! Read-only by default. `--allow-control` adds stop / pause / resume.
//! Starting jobs stays in the shell (`ptrack run -d ...`) on purpose.

use std::io::{self, BufRead, Write};
use std::path::Path;
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use crate::commands::{self, Stopped};
use crate::job::{self, Job, State};
use crate::supervise::summary;
use crate::util::{fmt_duration, last_lines, now_ms};

const DEFAULT_PROTOCOL: &str = "2025-06-18";

const INSTRUCTIONS: &str = "\
ptrack tracks long-running commands (builds, tests, servers). Start them in the shell with \
`ptrack run -d -l \"<label>\" -- <command>`; it prints a job id. Then use these tools to check \
on them cheaply instead of reading log files: jobs_list for an overview, job_status for one \
job, job_tail for recent output, job_wait to block until a job ends.";

pub fn serve(allow_control: bool) -> i32 {
    let stdin = io::stdin();
    let mut out = io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle(&msg, allow_control),
            Err(e) => Some(error_reply(
                Value::Null,
                -32700,
                &format!("parse error: {e}"),
            )),
        };
        if let Some(reply) = reply {
            let _ = writeln!(out, "{reply}");
            let _ = out.flush();
        }
    }
    0
}

fn handle(msg: &Value, allow_control: bool) -> Option<Value> {
    // Notifications have no id and get no reply.
    let id = msg.get("id")?.clone();
    let method = msg["method"].as_str().unwrap_or("");
    let params = &msg["params"];
    let result = match method {
        "initialize" => json!({
            "protocolVersion": params["protocolVersion"].as_str().unwrap_or(DEFAULT_PROTOCOL),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "ptrack", "version": env!("CARGO_PKG_VERSION") },
            "instructions": INSTRUCTIONS,
        }),
        "ping" => json!({}),
        "tools/list" => json!({ "tools": tool_list(allow_control) }),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or("");
            let args = &params["arguments"];
            match call_tool(name, args, allow_control) {
                Ok(text) => {
                    json!({ "content": [{ "type": "text", "text": text }], "isError": false })
                }
                Err(text) => {
                    json!({ "content": [{ "type": "text", "text": text }], "isError": true })
                }
            }
        }
        _ => {
            return Some(error_reply(
                id,
                -32601,
                &format!("method not found: {method}"),
            ));
        }
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn error_reply(id: Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn id_param() -> Value {
    json!({ "type": "string", "description": "Job id, a unique prefix of it, or 'last'." })
}

fn tool_list(allow_control: bool) -> Vec<Value> {
    let mut tools = vec![
        json!({
            "name": "jobs_list",
            "description": "List running jobs and recently finished ones, one line each: id, state, duration, label or command, agent, last output line.",
            "inputSchema": { "type": "object", "properties": {
                "all": { "type": "boolean", "description": "Include old finished jobs too." },
                "cwd": { "type": "string", "description": "Only jobs started in this folder or below." }
            }},
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "job_status",
            "description": "Full record of one job: state, exit code, command, folder, timings, agent, log path, last output line.",
            "inputSchema": { "type": "object", "properties": { "id": id_param() }, "required": ["id"] },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "job_tail",
            "description": "The last lines of a job's output.",
            "inputSchema": { "type": "object", "properties": {
                "id": id_param(),
                "lines": { "type": "integer", "description": "How many lines (default 40, max 500)." }
            }, "required": ["id"] },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "job_wait",
            "description": "Wait until a job ends (or the timeout passes) and return its result, with the last 20 output lines if it did not pass.",
            "inputSchema": { "type": "object", "properties": {
                "id": id_param(),
                "timeout_s": { "type": "integer", "description": "Seconds to wait (default 60, max 600)." }
            }, "required": ["id"] },
            "annotations": { "readOnlyHint": true }
        }),
    ];
    if allow_control {
        tools.push(json!({
            "name": "job_stop",
            "description": "Stop a job and every process it started. It first gets a grace period to exit on its own.",
            "inputSchema": { "type": "object", "properties": {
                "id": id_param(),
                "grace_s": { "type": "integer", "description": "Seconds to wait before forcing (default 5; 0 = force now)." }
            }, "required": ["id"] },
            "annotations": { "destructiveHint": true }
        }));
        tools.push(json!({
            "name": "job_pause",
            "description": "Freeze every process of a running job. Network connections inside it may time out while paused.",
            "inputSchema": { "type": "object", "properties": { "id": id_param() }, "required": ["id"] }
        }));
        tools.push(json!({
            "name": "job_resume",
            "description": "Unfreeze a paused job.",
            "inputSchema": { "type": "object", "properties": { "id": id_param() }, "required": ["id"] }
        }));
        tools.push(json!({
            "name": "job_rerun",
            "description": "Run a finished job's command again, in the same folder, as a new background job. Returns the new job id.",
            "inputSchema": { "type": "object", "properties": { "id": id_param() }, "required": ["id"] }
        }));
    }
    tools
}

fn call_tool(name: &str, args: &Value, allow_control: bool) -> Result<String, String> {
    let id = || -> Result<Job, String> {
        let id = args["id"].as_str().ok_or("missing 'id'")?;
        job::resolve(Some(id))
    };
    let control = |what: &str| -> Result<(), String> {
        if allow_control {
            Ok(())
        } else {
            Err(format!(
                "{what} is disabled; the user can enable it by starting the server with `ptrack mcp --allow-control`"
            ))
        }
    };
    match name {
        "jobs_list" => {
            let (jobs, total) = commands::visible_jobs(
                args["all"].as_bool().unwrap_or(false),
                args["cwd"].as_str(),
            );
            if jobs.is_empty() {
                return Ok(format!("no running or recent jobs ({total} older)"));
            }
            let mut out: Vec<String> = jobs.iter().map(one_line).collect();
            if jobs.len() < total {
                out.push(format!(
                    "({} older jobs hidden; pass all=true)",
                    total - jobs.len()
                ));
            }
            Ok(out.join("\n"))
        }
        "job_status" => {
            let j = id()?;
            Ok(commands::to_json(&j).to_string())
        }
        "job_tail" => {
            let j = id()?;
            let n = args["lines"].as_u64().unwrap_or(40).min(500) as usize;
            let lines = last_lines(Path::new(&j.log_path), n).unwrap_or_default();
            let state = j.effective_state().as_str();
            Ok(format!("[job {} {state}]\n{}", j.id, lines.join("\n")))
        }
        "job_wait" => {
            let start = id()?;
            let timeout = args["timeout_s"].as_u64().unwrap_or(60).min(600);
            let deadline = now_ms() + timeout * 1000;
            loop {
                let j = Job::load(&start.id).map_err(|e| e.to_string())?;
                let state = j.effective_state();
                if !state.is_active() {
                    let mut text = summary(&j);
                    if state != State::Passed {
                        let lines = last_lines(Path::new(&j.log_path), 20).unwrap_or_default();
                        text.push('\n');
                        text.push_str(&lines.join("\n"));
                    }
                    return Ok(text);
                }
                if now_ms() >= deadline {
                    return Ok(format!(
                        "still {} after waiting {timeout}s: {}",
                        state.as_str(),
                        one_line(&j)
                    ));
                }
                thread::sleep(Duration::from_millis(500));
            }
        }
        "job_stop" => {
            control("job_stop")?;
            let j = id()?;
            let grace = args["grace_s"].as_u64().unwrap_or(5);
            Ok(match commands::stop_job(&j, grace * 1000)? {
                Stopped::Already(d) => {
                    format!("job {} had already {}", d.id, d.effective_state().as_str())
                }
                Stopped::Now(d) => summary(&d),
            })
        }
        "job_pause" => {
            control("job_pause")?;
            let j = id()?;
            Ok(if commands::pause_job(&j)? {
                format!("paused job {}", j.id)
            } else {
                format!("job {} was already paused", j.id)
            })
        }
        "job_resume" => {
            control("job_resume")?;
            let j = id()?;
            commands::resume_job(&j)?;
            Ok(format!("resumed job {}", j.id))
        }
        "job_rerun" => {
            control("job_rerun")?;
            let j = id()?;
            commands::rerun_detached(&j)
        }
        _ => Err(format!("unknown tool '{name}'")),
    }
}

/// `k3f9a2 running 3m04s "full build" [claude-code] · > Task :app:compileJava`
fn one_line(j: &Job) -> String {
    let mut s = format!(
        "{} {} {} \"{}\"",
        j.id,
        j.effective_state().as_str(),
        fmt_duration(j.duration_ms()),
        j.title()
    );
    if let Some(code) = j.exit_code {
        s.push_str(&format!(" exit={code}"));
    }
    if let Some(a) = &j.agent {
        s.push_str(&format!(" [{}]", a.split('_').next().unwrap_or(a)));
    }
    if let Some(l) = &j.last_line {
        s.push_str(" · ");
        s.push_str(l);
    }
    s
}
