//! Claude Code integration: `ptrack hook claude-code` is called by Claude
//! Code's hooks, and `ptrack hook install` registers it in settings.json.
//!
//! - PreToolUse (Bash): long-running commands (and every background command)
//!   are rewritten to run through `ptrack run`, labelled with the tool call's
//!   description and tagged with the session and prompt ids. No other change:
//!   the agent sees the same output and exit code. The hook picks the job id
//!   itself, so its reply can tell the user (`systemMessage`) how to follow
//!   the job before it even starts.
//! - UserPromptSubmit: keeps a short preview of each prompt, so jobs can show
//!   which request started them.
//!
//! The hook never approves anything: permission decisions stay with Claude
//! Code and the user. Any error means "do nothing", never "block".

use std::fs;
use std::io::Read;
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::job;
use crate::util::truncate;

/// Programs that are long-running whatever their arguments.
const LONG_PROGRAMS: &[&str] = &[
    "ant",
    "bazel",
    "cmake",
    "cypress",
    "flutter",
    "gradle",
    "gradlew",
    "jest",
    "make",
    "mocha",
    "msbuild",
    "mvn",
    "mvnw",
    "ninja",
    "nox",
    "playwright",
    "pytest",
    "rspec",
    "sbt",
    "tox",
    "tsc",
    "vitest",
    "webpack",
    "xcodebuild",
];

/// Program + first argument pairs that are long-running.
const LONG_SUBCOMMANDS: &[(&str, &[&str])] = &[
    (
        "npm",
        &[
            "install", "i", "ci", "test", "t", "run", "start", "build", "exec",
        ],
    ),
    ("yarn", &["install", "test", "run", "build", "start", "dev"]),
    (
        "pnpm",
        &[
            "install", "i", "test", "run", "build", "start", "dev", "exec",
        ],
    ),
    ("bun", &["install", "test", "run", "build"]),
    ("deno", &["test", "task", "run", "compile"]),
    (
        "cargo",
        &[
            "build", "b", "test", "t", "run", "r", "check", "c", "clippy", "bench", "install",
            "nextest", "doc",
        ],
    ),
    ("go", &["build", "test", "run", "generate", "install"]),
    ("dotnet", &["build", "test", "run", "restore", "publish"]),
    ("docker", &["build", "compose", "run", "pull", "push"]),
    ("docker-compose", &["up", "build", "run", "pull"]),
    ("pip", &["install"]),
    ("pip3", &["install"]),
    ("uv", &["sync", "pip", "run"]),
    ("poetry", &["install", "run"]),
    ("python", &["-m"]),
    ("python3", &["-m"]),
    ("py", &["-m"]),
    ("terraform", &["init", "plan", "apply"]),
    ("swift", &["build", "test"]),
    ("mix", &["test", "compile", "deps.get"]),
    ("composer", &["install", "update"]),
    ("bundle", &["install", "exec"]),
];

/// Runners that execute another tool given as the next word.
const RUNNERS: &[&str] = &["npx", "pnpx", "bunx", "time", "nice", "nohup"];

/// Entry point for `ptrack hook claude-code`. Always exits 0.
pub fn run_claude_code(wrap_all: bool) -> i32 {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return 0;
    }
    let Ok(event) = serde_json::from_str::<Value>(&input) else {
        return 0;
    };
    match event["hook_event_name"].as_str() {
        Some("PreToolUse") => {
            if let Some(out) = pre_tool_use(&event, wrap_all) {
                println!("{out}");
            }
        }
        Some("UserPromptSubmit") => record_prompt(&event),
        _ => {}
    }
    0
}

fn pre_tool_use(event: &Value, wrap_all: bool) -> Option<Value> {
    if event["tool_name"].as_str()? != "Bash" {
        return None;
    }
    let input = &event["tool_input"];
    let command = input["command"].as_str()?;
    let background = input["run_in_background"].as_bool().unwrap_or(false);
    if !should_wrap(command, background, wrap_all) {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let id = job::fresh_id();
    let wrapped = wrap(
        &exe.display().to_string(),
        &id,
        command,
        input["description"].as_str(),
        event["session_id"].as_str(),
        event["prompt_id"].as_str(),
    );
    // updatedInput replaces the whole tool input, so keep the other fields.
    let mut updated = input.clone();
    updated["command"] = Value::String(wrapped);
    Some(json!({
        // Shown to the user only; the agent sees the job id in the output.
        "systemMessage": follow_hint(&id),
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "updatedInput": updated,
        }
    }))
}

pub fn should_wrap(command: &str, background: bool, wrap_all: bool) -> bool {
    if command.contains("ptrack") || command.trim().is_empty() {
        return false;
    }
    if background || wrap_all {
        return true;
    }
    command.split(['\n', ';', '|', '&']).any(is_long_running)
}

fn is_long_running(segment: &str) -> bool {
    let mut words = segment
        .split_whitespace()
        .skip_while(|w| w.contains('=') && !w.starts_with('-')) // FOO=bar env prefixes
        .map(|w| w.trim_matches(|c| c == '"' || c == '\''));
    let mut program = match words.next() {
        Some(w) => program_name(w),
        None => return false,
    };
    while RUNNERS.contains(&program.as_str()) {
        match words.next() {
            Some(w) => program = program_name(w),
            None => return false,
        }
    }
    if LONG_PROGRAMS.contains(&program.as_str()) {
        return true;
    }
    let first_arg = words.next().unwrap_or("");
    // `yarn` alone installs dependencies.
    if program == "yarn" && first_arg.is_empty() {
        return true;
    }
    LONG_SUBCOMMANDS
        .iter()
        .any(|(p, subs)| *p == program && subs.contains(&first_arg))
}

/// `./gradlew.bat` -> `gradlew`, `C:\x\npm.cmd` -> `npm`.
fn program_name(word: &str) -> String {
    let base = word.rsplit(['/', '\\']).next().unwrap_or(word);
    let lower = base.to_ascii_lowercase();
    for ext in [".exe", ".cmd", ".bat", ".sh", ".ps1"] {
        if let Some(stem) = lower.strip_suffix(ext) {
            return stem.to_string();
        }
    }
    lower
}

/// How to follow a job, for the user. Claude Code shows it after its own
/// "PreToolUse:Bash says:" prefix, so it is a plain sentence, not a list.
pub fn follow_hint(id: &str) -> String {
    format!(
        "This command runs as ptrack job {id}. To watch it live, run \"ptrack tail {id} -f\" \
         in a terminal, or open it in your browser with \"ptrack ui --job {id}\"."
    )
}

/// Builds `'<ptrack>' run --job-id <id> -l '<label>' --session .. --prompt-id .. -- "$BASH" -c '<command>'`.
/// The command keeps running in the same kind of shell the agent uses.
pub fn wrap(
    exe: &str,
    id: &str,
    command: &str,
    label: Option<&str>,
    session: Option<&str>,
    prompt_id: Option<&str>,
) -> String {
    let mut parts = vec![
        sh_quote(&exe.replace('\\', "/")),
        "run".to_string(),
        "--job-id".to_string(),
        sh_quote(id),
    ];
    let label = label.filter(|l| !l.trim().is_empty());
    for (flag, value) in [
        ("-l", label),
        ("--session", session),
        ("--prompt-id", prompt_id),
    ] {
        if let Some(v) = value {
            parts.push(flag.to_string());
            parts.push(sh_quote(&truncate(v, 80)));
        }
    }
    parts.push("--".to_string());
    parts.push("\"${BASH:-${SHELL:-sh}}\"".to_string());
    parts.push("-c".to_string());
    // Git Bash rewrites arguments that start with '/' into Windows paths when
    // calling a native program; a leading space keeps the script untouched.
    let script = if cfg!(windows) {
        format!(" {command}")
    } else {
        command.to_string()
    };
    parts.push(sh_quote(&script));
    parts.join(" ")
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn record_prompt(event: &Value) {
    let (Some(id), Some(prompt)) = (event["prompt_id"].as_str(), event["prompt"].as_str()) else {
        return;
    };
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return;
    }
    let one_line = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    let dir = job::prompts_dir();
    if fs::create_dir_all(&dir).is_ok() {
        let _ = fs::write(dir.join(format!("{id}.txt")), truncate(&one_line, 200));
    }
}

// ---------------------------------------------------------------- install

pub struct InstallOpts {
    pub project: bool,
    pub wrap_all: bool,
    pub record_prompts: bool,
}

fn settings_path(project: bool) -> Result<PathBuf, String> {
    if project {
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        return Ok(cwd.join(".claude").join("settings.local.json"));
    }
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return Ok(PathBuf::from(dir).join("settings.json"));
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or("cannot find your home folder")?;
    Ok(PathBuf::from(home).join(".claude").join("settings.json"))
}

fn is_ours(entry: &Value) -> bool {
    entry["hooks"].as_array().is_some_and(|hooks| {
        hooks.iter().any(|h| {
            h["command"]
                .as_str()
                .is_some_and(|c| c.contains("ptrack") && c.contains("hook claude-code"))
        })
    })
}

fn load_settings(path: &PathBuf) -> Result<Value, String> {
    match fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(json!({})),
        Ok(text) => serde_json::from_str(&text).map_err(|e| {
            format!(
                "{} is not valid JSON ({e}); fix it first, nothing was changed",
                path.display()
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

fn save_settings(path: &PathBuf, settings: &Value) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    if path.exists() {
        let backup = path.with_extension("json.ptrack-backup");
        fs::copy(path, &backup).map_err(|e| format!("cannot back up settings: {e}"))?;
    }
    let text = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    fs::write(path, text + "\n").map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Removes our entries from every hook event; returns how many were removed.
fn remove_ours(settings: &mut Value) -> usize {
    let mut removed = 0;
    if let Some(hooks) = settings["hooks"].as_object_mut() {
        for entries in hooks.values_mut() {
            if let Some(list) = entries.as_array_mut() {
                let before = list.len();
                list.retain(|e| !is_ours(e));
                removed += before - list.len();
            }
        }
        hooks.retain(|_, v| v.as_array().is_none_or(|a| !a.is_empty()));
    }
    if settings["hooks"].as_object().is_some_and(|h| h.is_empty()) {
        settings.as_object_mut().map(|o| o.remove("hooks"));
    }
    removed
}

pub fn install(opts: InstallOpts) -> Result<i32, String> {
    let path = settings_path(opts.project)?;
    let mut settings = load_settings(&path)?;
    if !settings.is_object() {
        return Err(format!("{} does not contain a JSON object", path.display()));
    }
    remove_ours(&mut settings);

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut command = format!(
        "\"{}\" hook claude-code",
        exe.display().to_string().replace('\\', "/")
    );
    if opts.wrap_all {
        command.push_str(" --all");
    }
    let handler = json!([{ "type": "command", "command": command }]);
    if !settings["hooks"].is_object() {
        settings["hooks"] = json!({});
    }
    let hooks = &mut settings["hooks"];
    let mut add = |event: &str, entry: Value| {
        if !hooks[event].is_array() {
            hooks[event] = json!([]);
        }
        hooks[event].as_array_mut().unwrap().push(entry);
    };
    add(
        "PreToolUse",
        json!({ "matcher": "Bash", "hooks": handler.clone() }),
    );
    if opts.record_prompts {
        add("UserPromptSubmit", json!({ "hooks": handler }));
    }
    save_settings(&path, &settings)?;

    println!(
        "Installed the ptrack hook for Claude Code in {}",
        path.display()
    );
    println!(
        "  {} through ptrack; prompts {}.",
        if opts.wrap_all {
            "every Bash command now runs"
        } else {
            "long-running and background commands now run"
        },
        if opts.record_prompts {
            "are linked to their jobs (first 200 characters kept locally)"
        } else {
            "are not recorded"
        }
    );
    println!("  Restart Claude Code (or open /hooks) for it to take effect.");
    println!(
        "  Undo with: ptrack hook uninstall{}",
        if opts.project { " --project" } else { "" }
    );
    Ok(0)
}

pub fn uninstall(project: bool) -> Result<i32, String> {
    let path = settings_path(project)?;
    if !path.exists() {
        println!("Nothing to remove: {} does not exist", path.display());
        return Ok(0);
    }
    let mut settings = load_settings(&path)?;
    let removed = remove_ours(&mut settings);
    if removed == 0 {
        println!("No ptrack hook found in {}", path.display());
        return Ok(0);
    }
    save_settings(&path, &settings)?;
    println!("Removed the ptrack hook from {}", path.display());
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_long_commands_only() {
        for cmd in [
            "npm test",
            "cd app && ./gradlew build",
            "CI=1 npx jest --watch=false",
            "cargo build --release 2>&1 | tail -5",
            "python -m pytest tests/",
            "yarn",
            "C:/tools/mvn.cmd package",
        ] {
            assert!(should_wrap(cmd, false, false), "{cmd}");
        }
        for cmd in [
            "ls -la",
            "git status",
            "npm --version",
            "cat Cargo.toml",
            "echo make",
            "ptrack list",
        ] {
            assert!(!should_wrap(cmd, false, false), "{cmd}");
        }
        assert!(
            should_wrap("ls", true, false),
            "background commands are always wrapped"
        );
        assert!(should_wrap("ls", false, true), "--all wraps everything");
        assert!(
            !should_wrap("ptrack run -- make", true, true),
            "never wrap twice"
        );
    }

    #[test]
    fn wrap_quotes_everything() {
        let w = wrap(
            "C:\\p\\ptrack.exe",
            "ab12cd",
            "echo 'hi' && make",
            Some("Build it's thing"),
            Some("s1"),
            None,
        );
        assert!(w.starts_with("'C:/p/ptrack.exe' run --job-id 'ab12cd' -l 'Build it'\\''s thing' --session 's1' -- \"${BASH:-${SHELL:-sh}}\" -c '"));
        assert!(w.ends_with("echo '\\''hi'\\'' && make'"));
    }

    #[test]
    fn install_and_uninstall_keep_other_settings() {
        let mut s = json!({
            "model": "x",
            "hooks": { "PreToolUse": [ { "matcher": "Edit", "hooks": [ { "type": "command", "command": "fmt.sh" } ] } ] }
        });
        s["hooks"]["PreToolUse"].as_array_mut().unwrap().push(json!({
            "matcher": "Bash", "hooks": [ { "type": "command", "command": "\"/x/ptrack\" hook claude-code" } ]
        }));
        assert_eq!(remove_ours(&mut s), 1);
        assert_eq!(s["model"], "x");
        assert_eq!(s["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
        let mut only_ours = json!({ "hooks": { "UserPromptSubmit": [ { "hooks": [ { "type": "command", "command": "ptrack hook claude-code" } ] } ] } });
        assert_eq!(remove_ours(&mut only_ours), 1);
        assert_eq!(only_ours, json!({}));
    }
}
