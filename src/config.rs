//! User settings, kept in `<home>/config.json`. Only `ptrack notify ...`
//! writes this file. A missing file means "all defaults".

use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::job;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Config {
    pub notify: NotifyConfig,
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
