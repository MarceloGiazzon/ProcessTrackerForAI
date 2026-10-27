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
