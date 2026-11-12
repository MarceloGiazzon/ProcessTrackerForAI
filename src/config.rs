//! User settings, kept in `<home>/config.json`. Only `ptrack notify ...` and
//! `ptrack config ...` write this file. A missing file means "all defaults".

use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::job;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Config {
    pub notify: NotifyConfig,
    pub activity: ActivityConfig,
}

/// How ptrack reports on jobs that print nothing for a while.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct ActivityConfig {
    /// After this many seconds without output, the supervisor writes a
    /// `# [ptrack] ...` line into the log, and again every so many seconds
    /// while the silence lasts. 0 turns these lines off.
    pub heartbeat_seconds: u64,
    /// A running job that printed nothing for this long is `quiet` (still
    /// using CPU) or, later, `stalled?`.
    pub quiet_after: u64,
    /// No output and no CPU for this long: `stalled?`. Advice only.
    pub stalled_after: u64,
}

impl Default for ActivityConfig {
    fn default() -> Self {
        ActivityConfig {
            heartbeat_seconds: 30,
            quiet_after: 60,
            stalled_after: 300,
        }
    }
}

/// Seconds between heartbeat lines: `PTRACK_HEARTBEAT_SECONDS` if set to a
/// number, else the setting.
pub fn heartbeat_seconds(cfg: &Config) -> u64 {
    std::env::var("PTRACK_HEARTBEAT_SECONDS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(cfg.activity.heartbeat_seconds)
}

const USAGE: &str =
    "usage: ptrack config [heartbeat-seconds N | quiet-after SECS | stalled-after SECS]";

/// `ptrack config`: shows the activity settings, or changes one.
pub fn command(args: &[String]) -> Result<i32, String> {
    let mut cfg = load_checked()?;
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let number = |name: &str, n: &str| -> Result<u64, String> {
        n.parse()
            .map_err(|_| format!("{name} needs a whole number of seconds, got '{n}'"))
    };
    match words.as_slice() {
        [] => {
            show(&cfg);
            return Ok(0);
        }
        ["heartbeat-seconds", n] => {
            cfg.activity.heartbeat_seconds = number("heartbeat-seconds", n)?
        }
        [name @ ("quiet-after" | "stalled-after"), n] => {
            let secs = number(name, n)?;
            if secs == 0 {
                return Err(format!("{name} must be at least 1 second"));
            }
            if *name == "quiet-after" {
                cfg.activity.quiet_after = secs;
            } else {
                cfg.activity.stalled_after = secs;
            }
        }
        _ => return Err(USAGE.into()),
    }
    save(&cfg).map_err(|e| format!("cannot save settings: {e}"))?;
    show(&cfg);
    Ok(0)
}

fn show(cfg: &Config) {
    let a = &cfg.activity;
    let heartbeat = match a.heartbeat_seconds {
        0 => "off".to_string(),
        n => format!("every {n}s while a job prints nothing"),
    };
    println!("heartbeat-seconds: {} ({heartbeat})", a.heartbeat_seconds);
    if let Ok(v) = std::env::var("PTRACK_HEARTBEAT_SECONDS") {
        println!("  PTRACK_HEARTBEAT_SECONDS={v} is set and wins over this setting");
    }
    println!(
        "quiet-after: {} (a running job silent this long is 'quiet' while it still uses CPU)",
        a.quiet_after
    );
    println!(
        "stalled-after: {} (no output and no CPU this long: 'stalled?', probably hung)",
        a.stalled_after
    );
    println!("settings file: {}", path().display());
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct NotifyConfig {
    /// Show a desktop notification when a job ends.
    pub desktop: bool,
    /// Only jobs that ran at least this many seconds trigger notifications.
    pub min_seconds: u64,
    /// Also POST each finished job to this http(s) URL.
    pub webhook: Option<String>,
    pub webhook_format: WebhookFormat,
}

impl Default for NotifyConfig {
    fn default() -> Self {
        NotifyConfig {
            desktop: false,
            min_seconds: 30,
            webhook: None,
            webhook_format: WebhookFormat::Json,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum WebhookFormat {
    /// `{"text": ..., "content": ..., "job": {...}}`: Slack reads `text`,
    /// Discord reads `content`, scripts read `job`.
    #[default]
    Json,
    /// Two plain lines (title, details), e.g. for ntfy.sh.
    Text,
}

pub fn path() -> PathBuf {
    job::home().join("config.json")
}

/// Settings for the supervisor: any problem reading them means defaults, so
/// a broken settings file can never break a job.
pub fn load() -> Config {
    load_checked().unwrap_or_default()
}

/// Settings for commands that change them: a broken file is reported instead
/// of being silently replaced.
pub fn load_checked() -> Result<Config, String> {
    match fs::read(path()) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
            format!(
                "{} is not valid ({e}); fix or delete it first",
                path().display()
            )
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(format!("cannot read {}: {e}", path().display())),
    }
}

pub fn save(config: &Config) -> io::Result<()> {
    fs::create_dir_all(job::home())?;
    let tmp = job::home().join("config.json.tmp");
    let data = serde_json::to_vec_pretty(config).map_err(io::Error::other)?;
    fs::write(&tmp, data)?;
    fs::rename(&tmp, path())
}
