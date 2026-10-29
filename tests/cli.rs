//! End-to-end tests: they run the real `ptrack` binary against a private data
//! folder, on whatever OS the tests run on.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::thread::sleep;
use std::time::Duration;

#[cfg(windows)]
const LONG: &str = "ping -n 60 127.0.0.1 > nul";
#[cfg(unix)]
const LONG: &str = "sleep 60";

struct Home(PathBuf);

impl Home {
    fn new(name: &str) -> Home {
        let dir = std::env::temp_dir().join(format!("ptrack-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Home(dir)
    }

    fn ptrack(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_ptrack"))
            .args(args)
            .env("PTRACK_HOME", &self.0)
            .output()
            .expect("ptrack runs")
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        let out = self.ptrack(args);
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "bad json from {args:?}: {e}\n{}",
                String::from_utf8_lossy(&out.stdout)
            )
        })
    }

    fn state(&self, id: &str) -> String {
        self.json(&["status", id, "--json"])["state"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Starts a background job and returns its id.
    fn detach(&self, script: &str) -> String {
        let out = self.ptrack(&["run", "-d", "-s", "--", script]);
        assert!(out.status.success(), "{out:?}");
        self.json(&["status", "last", "--json"])["id"]
            .as_str()
            .unwrap()
            .to_string()
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn records_pass_and_fail_with_exit_codes() {
    let h = Home::new("passfail");
    let ok = h.ptrack(&["run", "-q", "-s", "--", "echo hello"]);
    assert_eq!(ok.status.code(), Some(0));
    let bad = h.ptrack(&[
        "run",
        "-q",
        "-l",
        "broken",
        "-s",
        "--",
        "echo oops && exit 3",
    ]);
    assert_eq!(bad.status.code(), Some(3));

    let jobs = h.json(&["list", "--json"]);
    let jobs = jobs.as_array().unwrap();
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0]["state"], "failed");
    assert_eq!(jobs[0]["exit_code"], 3);
    assert_eq!(jobs[0]["label"], "broken");
    assert_eq!(jobs[0]["last_line"], "oops");
    assert_eq!(jobs[1]["state"], "passed");
    assert_eq!(jobs[1]["last_line"], "hello");
}

#[test]
fn project_and_tag_are_recorded_listed_and_filtered() {
    let h = Home::new("project");
    let run_in = |dir: &std::path::Path, args: &[&str], env: &[(&str, &str)]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_ptrack"));
        cmd.args(args).current_dir(dir).env("PTRACK_HOME", &h.0);
        for (k, v) in env {
            cmd.env(k, v);
        }
        let out = cmd.output().unwrap();
        assert!(out.status.success(), "{out:?}");
    };

    // Started deep inside a repository: the project is the repository's name.
    let repo = h.0.join("NPDev");
    let sub = repo.join("src").join("app");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(&sub).unwrap();
    run_in(
        &sub,
        &["run", "-q", "--tag", "CI Watch", "-s", "--", "echo ok"],
        &[],
    );
    // The option wins over the folder, and PTRACK_TAG sets a tag too.
    run_in(
        &h.0,
        &["run", "-q", "--project", "Other", "-s", "--", "echo other"],
        &[("PTRACK_TAG", "nightly")],
    );

    let all = h.json(&["list", "--json"]);
    let all = all.as_array().unwrap();
    assert_eq!(all[0]["project"], "Other");
    assert_eq!(all[0]["tag"], "nightly");
    assert_eq!(all[1]["project"], "NPDev");
    assert_eq!(all[1]["tag"], "CI Watch");

    let ids = |args: &[&str]| -> Vec<String> {
        h.json(args)
            .as_array()
            .unwrap()
            .iter()
            .map(|j| j["id"].as_str().unwrap().to_string())
            .collect()
    };
    let ci_job = all[1]["id"].as_str().unwrap().to_string();
    assert_eq!(
        ids(&["list", "--tag", "ci watch", "--json"]),
        std::slice::from_ref(&ci_job)
    );
    assert_eq!(
        ids(&["list", "--project", "npdev", "--json"]),
        std::slice::from_ref(&ci_job)
    );
    assert!(ids(&["list", "--project", "nope", "--json"]).is_empty());

    let table = String::from_utf8_lossy(&h.ptrack(&["list"]).stdout).to_string();
    assert!(
        table.contains("PROJECT") && table.contains("NPDev") && table.contains("CI Watch"),
        "{table}"
    );

    // A rerun keeps both.
    let out = h.ptrack(&["rerun", &ci_job, "-q"]);
    assert!(out.status.success(), "{out:?}");
    let again = h.json(&["status", "--json"]);
    assert_eq!(again["rerun_of"], ci_job.as_str());
    assert_eq!(again["project"], "NPDev");
    assert_eq!(again["tag"], "CI Watch");
}

/// Windows exit codes can be negative (npm exits with -4058). ptrack's own
/// exit code must still say "failed", not wrap or clamp to 0.
#[cfg(windows)]
#[test]
fn a_negative_exit_code_still_fails() {
    let h = Home::new("negative");
    let out = h.ptrack(&["run", "-q", "-s", "--", "exit -4058"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert_eq!(h.state("last"), "failed");
    assert_eq!(h.json(&["status", "--json"])["exit_code"], -4058);
}

#[test]
fn agents_are_recognised_by_the_variables_they_set() {
    let h = Home::new("agents");
    // What this test itself may be running under must not leak in.
    const ALL: &[&str] = &[
        "PTRACK_AGENT",
        "PTRACK_SESSION",
        "AI_AGENT",
        "CLAUDECODE",
        "CLAUDE_CODE_SESSION_ID",
        "CODEX_THREAD_ID",
        "CODEX_SANDBOX",
        "CODEX_CI",
        "CURSOR_AGENT",
        "ANTIGRAVITY_AGENT",
        "GEMINI_CLI",
    ];
    let started_by = |env: &[(&str, &str)]| -> (serde_json::Value, serde_json::Value) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_ptrack"));
        cmd.args(["run", "-q", "-s", "--", "echo hi"])
            .env("PTRACK_HOME", &h.0);
        for var in ALL {
            cmd.env_remove(var);
        }
        for (k, v) in env {
            cmd.env(k, v);
        }
        assert!(cmd.output().unwrap().status.success());
        let j = h.json(&["status", "--json"]);
        (j["agent"].clone(), j["session"].clone())
    };

    let (agent, session) = started_by(&[("CODEX_THREAD_ID", "thread-7")]);
    assert_eq!(
        (agent.as_str(), session.as_str()),
        (Some("codex"), Some("thread-7"))
    );
    assert_eq!(started_by(&[("CURSOR_AGENT", "1")]).0, "cursor");
    assert_eq!(started_by(&[("GEMINI_CLI", "1")]).0, "gemini-cli");
    assert_eq!(started_by(&[("ANTIGRAVITY_AGENT", "1")]).0, "antigravity");
    // An explicit name wins over any marker.
    assert_eq!(
        started_by(&[("PTRACK_AGENT", "aider"), ("CODEX_SANDBOX", "seatbelt")]).0,
        "aider"
    );
    assert_eq!(started_by(&[]).0, serde_json::Value::Null);
}

#[test]
fn instructions_print_the_agent_block() {
    let h = Home::new("instructions");
    let out = h.ptrack(&["instructions"]);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.starts_with("## Long-running commands"), "{text}");
    assert!(text.contains("ptrack run -d") && text.contains("ptrack wait"));
    assert!(
        text.contains("ptrack ui --job <id>"),
        "tells the agent to show the user how to follow a job"
    );
    assert!(text.is_ascii(), "safe for any shell redirection");
}

#[test]
fn ui_can_open_on_one_job() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;

    let h = Home::new("uijob");
    let bad = h.ptrack(&["ui", "--no-open", "--port", "0", "--job", "nope"]);
    assert_eq!(
        bad.status.code(),
        Some(2),
        "unknown job fails before serving"
    );

    h.ptrack(&["run", "-q", "-s", "--", "echo hi"]);
    let id = h.json(&["status", "--json"])["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut child = Command::new(env!("CARGO_BIN_EXE_ptrack"))
        .args(["ui", "--no-open", "--port", "0", "--job", "last"])
        .env("PTRACK_HOME", &h.0)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut first = String::new();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    out.read_line(&mut first).unwrap();
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        first.trim().ends_with(&format!("&job={id}")),
        "`last` is resolved to the full id in the link: {first}"
    );
}

#[test]
fn hook_links_to_a_running_dashboard() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;

    let h = Home::new("hooklink");
    let hint = || -> String {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ptrack"))
            .args(["hook", "claude-code"])
            .env("PTRACK_HOME", &h.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let event = serde_json::json!({
            "hook_event_name": "PreToolUse", "tool_name": "Bash",
            "tool_input": { "command": "make", "description": "Build" }
        });
        write!(child.stdin.take().unwrap(), "{event}").unwrap();
        let out = child.wait_with_output().unwrap();
        let reply: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        reply["systemMessage"].as_str().unwrap().to_string()
    };
    assert!(
        hint().contains("open it in your browser with \"ptrack ui --job "),
        "no dashboard yet: the command, not a link"
    );

    let mut ui = Command::new(env!("CARGO_BIN_EXE_ptrack"))
        .args(["ui", "--no-open", "--port", "0"])
        .env("PTRACK_HOME", &h.0)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    // Keep the reader alive: a closed pipe would make the server's next
    // `println!` panic.
    let mut ui_out = BufReader::new(ui.stdout.take().unwrap());
    let mut first = String::new();
    ui_out.read_line(&mut first).unwrap();
    let base = first.trim().rsplit(' ').next().unwrap().to_string();

    let with_link = hint();
    let id = with_link
        .strip_prefix("This command runs as ptrack job ")
        .and_then(|rest| rest.split('.').next())
        .unwrap();
    assert!(
        with_link.contains(&format!("Watch it in your browser: {base}&job={id} ")),
        "{with_link}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(h.0.join("ui.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "the token file is private");
    }

    // A second `ptrack ui` reuses the running one instead of starting another.
    let again = h.ptrack(&["ui", "--no-open"]);
    let again = String::from_utf8_lossy(&again.stdout).to_string();
    assert!(
        again.contains("already running") && again.contains(&base),
        "{again}"
    );

    // Closed with Ctrl+C or killed: ui.json stays, but is not trusted.
    let _ = ui.kill();
    let _ = ui.wait();
    drop(ui_out);
    assert!(h.0.join("ui.json").exists());
    assert!(
        !hint().contains("http://"),
        "no link to a dashboard that is gone"
    );
}

#[test]
fn missing_program_fails_cleanly() {
    let h = Home::new("missing");
    let out = h.ptrack(&["run", "-q", "--", "definitely-not-a-real-program-xyz"]);
    assert_eq!(out.status.code(), Some(127));
    assert_eq!(h.state("last"), "failed");
}

#[test]
fn detach_then_wait_returns_job_exit_code() {
    let h = Home::new("wait");
    let id = h.detach("echo working && exit 4");
    let out = h.ptrack(&["wait", &id, "--timeout", "30"]);
    assert_eq!(out.status.code(), Some(4), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("failed"));
}

#[test]
fn stop_cancels_a_running_job() {
    let h = Home::new("stop");
    let id = h.detach(LONG);
    sleep(Duration::from_millis(800));
    assert_eq!(h.state(&id), "running");

    let out = h.ptrack(&["stop", &id, "--grace", "1"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(h.state(&id), "cancelled");
}

#[test]
fn pause_and_resume_change_state() {
    let h = Home::new("pause");
    let id = h.detach(LONG);
    sleep(Duration::from_millis(800));

    assert!(h.ptrack(&["pause", &id]).status.success());
    assert_eq!(h.state(&id), "paused");
    assert!(h.ptrack(&["resume", &id]).status.success());
    assert_eq!(h.state(&id), "running");

    // A paused job can still be stopped.
    assert!(h.ptrack(&["pause", &id]).status.success());
    assert!(h.ptrack(&["stop", &id, "--grace", "1"]).status.success());
    assert_eq!(h.state(&id), "cancelled");
}

#[test]
fn options_after_the_command_belong_to_the_command() {
    let h = Home::new("passthrough");
    // No `--`: the first word that is not a ptrack option starts the command.
    #[cfg(windows)]
    let args = ["run", "-q", "cmd", "/c", "echo", "--label", "x"];
    #[cfg(unix)]
    let args = ["run", "-q", "echo", "--label", "x"];
    assert!(h.ptrack(&args).status.success());
    let job = h.json(&["status", "--json"]);
    assert_eq!(job["label"], serde_json::Value::Null);
    assert_eq!(job["last_line"], "--label x");
}

#[test]
fn prune_removes_finished_jobs_only() {
    let h = Home::new("prune");
    h.ptrack(&["run", "-q", "-s", "--", "echo done"]);
    let id = h.detach(LONG);
    sleep(Duration::from_millis(500));
    assert!(h.ptrack(&["prune", "--all"]).status.success());
    let jobs = h.json(&["list", "--all", "--json"]);
    assert_eq!(jobs.as_array().unwrap().len(), 1);
    h.ptrack(&["stop", &id, "--force"]);
}

#[test]
fn mcp_server_lists_jobs_and_guards_control_tools() {
    use std::io::Write;
    use std::process::Stdio;

    let h = Home::new("mcp");
    h.ptrack(&[
        "run",
        "-q",
        "-l",
        "unit tests",
        "--project",
        "NPDev",
        "--tag",
        "CI Watch",
        "-s",
        "--",
        "echo 212 passed",
    ]);

    let session = |allow_control: bool, requests: &[serde_json::Value]| -> Vec<serde_json::Value> {
        let mut args = vec!["mcp"];
        if allow_control {
            args.push("--allow-control");
        }
        let mut child = Command::new(env!("CARGO_BIN_EXE_ptrack"))
            .args(&args)
            .env("PTRACK_HOME", &h.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        for r in requests {
            writeln!(stdin, "{r}").unwrap();
        }
        drop(stdin);
        let out = child.wait_with_output().unwrap();
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    };
    let call = |id: u32, name: &str, args: serde_json::Value| serde_json::json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":args}});

    let replies = session(
        false,
        &[
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}),
            serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
            call(3, "jobs_list", serde_json::json!({})),
            call(4, "job_stop", serde_json::json!({"id":"last"})),
            call(5, "jobs_list", serde_json::json!({"tag": "ci watch"})),
            call(6, "jobs_list", serde_json::json!({"project": "elsewhere"})),
        ],
    );
    assert_eq!(replies.len(), 6, "notifications get no reply: {replies:?}");
    assert_eq!(replies[0]["result"]["serverInfo"]["name"], "ptrack");
    let tools: Vec<&str> = replies[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(tools, ["jobs_list", "job_status", "job_tail", "job_wait"]);
    let listing = replies[2]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        listing.contains("passed")
            && listing.contains("unit tests")
            && listing.contains("212 passed")
            && listing.contains("project=NPDev")
            && listing.contains("tag=\"CI Watch\""),
        "{listing}"
    );
    assert_eq!(replies[3]["result"]["isError"], true);
    let by_tag = replies[4]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(by_tag.contains("unit tests"), "{by_tag}");
    let by_project = replies[5]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        by_project.starts_with("no running or recent jobs"),
        "{by_project}"
    );

    let replies = session(
        true,
        &[serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})],
    );
    assert_eq!(replies[0]["result"]["tools"].as_array().unwrap().len(), 8);
}

#[test]
fn dashboard_requires_token_and_local_host() {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpStream;
    use std::process::Stdio;

    let h = Home::new("ui");
    h.ptrack(&[
        "run",
        "-q",
        "--project",
        "NPDev",
        "--tag",
        "CI Watch",
        "-s",
        "--",
        "echo hi",
    ]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_ptrack"))
        .args(["ui", "--port", "0", "--no-open"])
        .env("PTRACK_HOME", &h.0)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut first = String::new();
    // Keep the reader alive: the server prints a second line, and a closed pipe
    // would make that `println!` panic and take the server down mid-test.
    let mut server_stdout = BufReader::new(child.stdout.take().unwrap());
    server_stdout.read_line(&mut first).unwrap();
    // "ptrack dashboard: http://127.0.0.1:<port>/#t=<token>"
    let url = first.trim().rsplit(' ').next().unwrap().to_string();
    let port: u16 = url
        .split(':')
        .nth(2)
        .unwrap()
        .split('/')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let token = url.split("#t=").nth(1).unwrap().to_string();

    let http = |method: &str, path: &str, host: &str, token: &str| -> (u16, String) {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(
            s,
            "{method} {path} HTTP/1.1\r\nHost: {host}\r\nX-Ptrack-Token: {token}\r\n\r\n"
        )
        .unwrap();
        let mut resp = String::new();
        s.read_to_string(&mut resp).unwrap();
        let code = resp[9..12].parse().unwrap();
        let body = resp
            .split_once("\r\n\r\n")
            .map(|(_, b)| b.to_string())
            .unwrap_or_default();
        (code, body)
    };
    let host = format!("127.0.0.1:{port}");

    let (code, page) = http("GET", "/", &host, "");
    assert_eq!(code, 200);
    assert!(page.contains("<title>ptrack jobs</title>"));
    assert!(
        page.contains("id=\"notify\""),
        "notification toggle is on the page"
    );
    assert_eq!(http("GET", "/api/jobs", &host, "").0, 401);
    assert_eq!(http("GET", "/api/jobs", &host, "wrong").0, 401);
    assert_eq!(
        http("GET", "/api/jobs", &format!("evil.example:{port}"), &token).0,
        403
    );
    let (code, body) = http("GET", "/api/jobs", &host, &token);
    assert_eq!(code, 200);
    let data: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(data["jobs"][0]["state"], "passed");
    assert_eq!(data["projects"], serde_json::json!(["NPDev"]));
    assert_eq!(data["tags"], serde_json::json!(["CI Watch"]));
    assert!(page.contains("id=\"project\"") && page.contains("id=\"tag\""));
    // Filters, with the tag encoded both ways browsers do it.
    let count = |path: &str| -> usize {
        let (code, body) = http("GET", path, &host, &token);
        assert_eq!(code, 200, "{body}");
        let data: serde_json::Value = serde_json::from_str(&body).unwrap();
        data["jobs"].as_array().unwrap().len()
    };
    assert_eq!(count("/api/jobs?tag=CI+Watch"), 1);
    assert_eq!(count("/api/jobs?tag=ci%20watch&project=npdev"), 1);
    assert_eq!(count("/api/jobs?project=elsewhere"), 0);
    assert_eq!(http("POST", "/api/jobs/last/resume", &host, &token).0, 409);
    let (code, body) = http("POST", "/api/jobs/last/rerun", &host, &token);
    assert_eq!(code, 200, "{body}");
    assert!(body.contains("rerun of"), "{body}");

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn claude_code_hook_wraps_long_commands_and_links_prompts() {
    use std::io::Write;
    use std::process::Stdio;

    let h = Home::new("hook");
    let hook = |event: serde_json::Value| -> String {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ptrack"))
            .args(["hook", "claude-code"])
            .env("PTRACK_HOME", &h.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        write!(child.stdin.take().unwrap(), "{event}").unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "the hook must never fail");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let prompt_id = "550e8400-e29b-41d4-a716-446655440000";

    let silent = hook(serde_json::json!({
        "hook_event_name": "UserPromptSubmit", "prompt_id": prompt_id, "session_id": "s1",
        "prompt": "Run the tests\nplease"
    }));
    assert_eq!(
        silent, "",
        "UserPromptSubmit output would be added to the conversation"
    );

    let untouched = hook(serde_json::json!({
        "hook_event_name": "PreToolUse", "tool_name": "Bash",
        "tool_input": { "command": "git status", "description": "status" }
    }));
    assert_eq!(untouched, "", "short commands are left alone");

    let garbage = hook(serde_json::json!("not an event"));
    assert_eq!(garbage, "");

    let out = hook(serde_json::json!({
        "hook_event_name": "PreToolUse", "tool_name": "Bash", "session_id": "s1", "prompt_id": prompt_id,
        "tool_input": { "command": "make test; echo done", "description": "Run tests", "timeout": 5000 }
    }));
    let reply: serde_json::Value = serde_json::from_str(&out).unwrap();
    let updated = &reply["hookSpecificOutput"]["updatedInput"];
    assert_eq!(reply["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    assert!(
        reply["hookSpecificOutput"]
            .get("permissionDecision")
            .is_none(),
        "never auto-approve"
    );
    assert_eq!(updated["timeout"], 5000, "other tool fields are kept");
    let command = updated["command"].as_str().unwrap();
    // The hook picks the job id, so it can tell the user how to follow the
    // job before it starts (systemMessage reaches the user, not the agent).
    let hint = reply["systemMessage"].as_str().unwrap();
    let id = hint
        .strip_prefix("This command runs as ptrack job ")
        .and_then(|rest| rest.split('.').next())
        .unwrap();
    assert!(
        hint.contains(&format!("ptrack tail {id} -f"))
            && hint.contains(&format!("ptrack ui --job {id}")),
        "{hint}"
    );
    assert!(
        command.contains(&format!(
            " run --job-id '{id}' -l 'Run tests' --session 's1' --prompt-id "
        )),
        "{command}"
    );

    // On Unix, run the rewritten command the way the agent's shell would.
    #[cfg(unix)]
    {
        let rewritten = command.replace("make test", "echo testing; exit 5");
        let status = Command::new("sh")
            .args(["-c", &rewritten])
            .env("PTRACK_HOME", &h.0)
            .status()
            .unwrap();
        assert_eq!(
            status.code(),
            Some(5),
            "the agent sees the command's own exit code"
        );
        let job = h.json(&["status", "--json"]);
        assert_eq!(job["id"], id, "the job got the id the user was shown");
        assert_eq!(job["label"], "Run tests");
        assert_eq!(job["prompt"], "Run the tests please");
        assert_eq!(job["state"], "failed");
    }
}

#[test]
fn rerun_repeats_the_command_in_the_same_folder() {
    let h = Home::new("rerun");
    let project = h.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let first = Command::new(env!("CARGO_BIN_EXE_ptrack"))
        .args([
            "run",
            "-q",
            "-l",
            "flaky",
            "-s",
            "--",
            "echo x> here.txt && exit 2",
        ])
        .current_dir(&project)
        .env("PTRACK_HOME", &h.0)
        .output()
        .unwrap();
    assert_eq!(first.status.code(), Some(2));
    let first = h.json(&["status", "--json"]);
    std::fs::remove_file(project.join("here.txt")).unwrap();

    // Rerun from a different folder: the command must still run in `project`.
    let out = h.ptrack(&["rerun", first["id"].as_str().unwrap(), "-q"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "rerun returns the command's exit code"
    );
    assert!(
        project.join("here.txt").exists(),
        "ran in the original folder"
    );

    let second = h.json(&["status", "--json"]);
    assert_ne!(second["id"], first["id"]);
    assert_eq!(second["rerun_of"], first["id"]);
    assert_eq!(second["label"], "flaky");
    assert_eq!(second["cwd"], first["cwd"]);
    assert_eq!(second["command"], first["command"]);
}

#[test]
fn rerun_refuses_a_job_that_is_still_running() {
    let h = Home::new("rerun-active");
    let id = h.detach(LONG);
    sleep(Duration::from_millis(500));
    let out = h.ptrack(&["rerun", &id]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("still running"));
    h.ptrack(&["stop", &id, "--force"]);
}

#[test]
fn notify_settings_are_saved_and_validated() {
    let h = Home::new("notify");
    let show = h.ptrack(&["notify"]);
    assert!(String::from_utf8_lossy(&show.stdout).contains("desktop notifications: off"));

    assert!(h.ptrack(&["notify", "on"]).status.success());
    assert!(h.ptrack(&["notify", "min-seconds", "0"]).status.success());
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(h.0.join("config.json")).unwrap()).unwrap();
    assert_eq!(saved["notify"]["desktop"], true);
    assert_eq!(saved["notify"]["min_seconds"], 0);

    assert_eq!(
        h.ptrack(&["notify", "min-seconds", "soon"]).status.code(),
        Some(2)
    );
    assert_eq!(h.ptrack(&["notify", "sideways"]).status.code(), Some(2));
    assert!(h.ptrack(&["notify", "off"]).status.success());

    // A broken settings file is reported, not overwritten, and never breaks jobs.
    std::fs::write(h.0.join("config.json"), "{ broken").unwrap();
    assert_eq!(h.ptrack(&["notify", "on"]).status.code(), Some(2));
    assert_eq!(
        std::fs::read_to_string(h.0.join("config.json")).unwrap(),
        "{ broken"
    );
    let out = h.ptrack(&["run", "-q", "-s", "--", "echo still works"]);
    assert!(out.status.success());
}

/// A one-request web server for webhook tests. Returns its URL and a channel
/// that receives the request body. With `respond: false` it never answers
/// (a hanging webhook).
fn fake_webhook(respond: bool) -> (String, std::sync::mpsc::Receiver<String>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/hook", listener.local_addr().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(15)))
            .unwrap();
        let mut data = Vec::new();
        let mut buf = [0u8; 4096];
        let body = loop {
            let n = stream.read(&mut buf).unwrap_or(0);
            if n == 0 {
                break String::new();
            }
            data.extend_from_slice(&buf[..n]);
            let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") else {
                continue;
            };
            let head = String::from_utf8_lossy(&data[..end]).to_ascii_lowercase();
            let len: usize = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0);
            if data.len() >= end + 4 + len {
                break String::from_utf8_lossy(&data[end + 4..end + 4 + len]).into_owned();
            }
        };
        if respond {
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        } else {
            sleep(Duration::from_secs(20));
        }
        let _ = tx.send(body);
    });
    (url, rx)
}

#[test]
fn webhook_receives_finished_jobs() {
    let h = Home::new("webhook");
    let (url, received) = fake_webhook(true);
    assert!(h.ptrack(&["notify", "webhook", &url]).status.success());
    assert!(h.ptrack(&["notify", "min-seconds", "0"]).status.success());

    let out = h.ptrack(&["run", "-q", "-l", "hooked", "-s", "--", "echo delivered"]);
    assert!(out.status.success());
    let body = received
        .recv_timeout(Duration::from_secs(20))
        .expect("webhook called");
    let payload: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(payload["job"]["label"], "hooked");
    assert_eq!(payload["job"]["state"], "passed");
    assert!(
        payload["text"]
            .as_str()
            .unwrap()
            .starts_with("Job passed: hooked")
    );
    assert_eq!(payload["content"], payload["text"]);
}

#[test]
fn webhook_test_command_reports_delivery_and_bad_urls_are_refused() {
    let h = Home::new("webhook-test");
    let (url, received) = fake_webhook(true);
    assert!(h.ptrack(&["notify", "webhook", &url]).status.success());
    assert!(
        h.ptrack(&["notify", "webhook-format", "text"])
            .status
            .success()
    );
    let out = h.ptrack(&["notify", "test"]);
    assert!(out.status.success(), "{out:?}");
    let body = received.recv_timeout(Duration::from_secs(20)).unwrap();
    assert!(body.starts_with("Job passed: ptrack test\n"), "{body}");

    for bad in [
        "file:///etc/passwd",
        "-o/tmp/x",
        "ftp://example.com",
        "http://a b",
    ] {
        assert_eq!(
            h.ptrack(&["notify", "webhook", bad]).status.code(),
            Some(2),
            "{bad}"
        );
    }
}

#[test]
fn a_hanging_webhook_does_not_delay_the_command() {
    let h = Home::new("webhook-slow");
    let (url, _received) = fake_webhook(false);
    assert!(h.ptrack(&["notify", "webhook", &url]).status.success());
    assert!(h.ptrack(&["notify", "min-seconds", "0"]).status.success());
    // `output()` waits until every copy of the output pipe is closed, exactly
    // like an agent capturing the command. curl must not hold it open.
    let started = std::time::Instant::now();
    let out = h.ptrack(&["run", "-q", "-s", "--", "echo quick"]);
    assert!(out.status.success());
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "took {:?}: the notifier kept the caller's pipe open",
        started.elapsed()
    );
}

/// True while any process of the group exists (signal 0 only checks).
#[cfg(unix)]
fn group_alive(pgid: i64) -> bool {
    // SAFETY: signal 0 delivers nothing.
    unsafe { libc::kill(-(pgid as i32), 0) == 0 }
}

#[cfg(unix)]
fn pid_alive(pid: i64) -> bool {
    // SAFETY: signal 0 delivers nothing.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(unix)]
fn eventually(mut condition: impl FnMut() -> bool, secs: u64) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    while std::time::Instant::now() < deadline {
        if condition() {
            return true;
        }
        sleep(Duration::from_millis(100));
    }
    condition()
}

/// Kills a background job's supervisor with SIGKILL (no chance to clean
/// up) and returns the job's process group id.
#[cfg(unix)]
fn kill_supervisor(h: &Home, id: &str) -> i64 {
    let job = h.json(&["status", id, "--json"]);
    let supervisor = job["supervisor_pid"].as_i64().unwrap();
    let pgid = job["pid"].as_i64().unwrap();
    assert!(group_alive(pgid), "job is running before the test");
    // SAFETY: plain syscall on a pid we just read from our own job file.
    assert_eq!(unsafe { libc::kill(supervisor as i32, libc::SIGKILL) }, 0);
    pgid
}

#[cfg(unix)]
#[test]
fn killing_the_supervisor_stops_the_job_on_unix() {
    let h = Home::new("guard");
    let id = h.detach(LONG);
    sleep(Duration::from_millis(800));
    let pgid = kill_supervisor(&h, &id);
    assert!(
        eventually(|| !group_alive(pgid), 10),
        "the job's processes outlived their supervisor"
    );
}

#[cfg(unix)]
#[test]
fn a_paused_job_is_cleaned_up_too_when_its_supervisor_dies() {
    let h = Home::new("guard-paused");
    let id = h.detach(LONG);
    sleep(Duration::from_millis(800));
    assert!(h.ptrack(&["pause", &id]).status.success());
    let pgid = kill_supervisor(&h, &id);
    assert!(
        eventually(|| !group_alive(pgid), 10),
        "a paused job outlived its supervisor"
    );
}

#[cfg(unix)]
#[test]
fn background_helpers_survive_a_normal_exit_on_unix() {
    let h = Home::new("guard-helper");
    let out = h.ptrack(&[
        "run",
        "-q",
        "-s",
        "--",
        "sleep 30 >/dev/null 2>&1 & echo $!",
    ]);
    assert!(out.status.success());
    let helper: i64 = h.json(&["status", "--json"])["last_line"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    sleep(Duration::from_millis(500));
    let alive = pid_alive(helper);
    // SAFETY: stops the helper this test started.
    unsafe { libc::kill(helper as i32, libc::SIGKILL) };
    assert!(
        alive,
        "a normal exit must not kill helpers the command left running"
    );
}
