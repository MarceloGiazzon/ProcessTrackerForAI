//! Notifications when a job ends. The job's supervisor calls `job_finished`
//! right after saving the final state. Everything here is best effort: a
//! failure never changes the job's result.
//!
//! Desktop notifications use a tool that already exists on each system, so
//! nothing has to be installed: PowerShell (Windows), osascript (macOS),
//! notify-send (Linux desktops). Job text is passed through environment
//! variables or arguments, never pasted into script source.
//!
//! Webhooks are sent with curl, which ships with Windows 10+, macOS and
//! practically every Linux. The payload goes through curl's stdin.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::json;

use crate::config::{self, Config, WebhookFormat};
use crate::job::{Job, State};
use crate::util::{fmt_duration, truncate};

/// Shows a Windows toast. Title and body come from environment variables, so
/// no job text is ever interpreted as PowerShell code. The app id is the one
/// Windows registers for PowerShell itself, which is allowed to show toasts.
#[cfg(windows)]
const WINDOWS_TOAST: &str = r#"
$ErrorActionPreference = 'Stop'
[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null
[Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom, ContentType = WindowsRuntime] | Out-Null
$xml = New-Object Windows.Data.Xml.Dom.XmlDocument
$xml.LoadXml('<toast><visual><binding template="ToastGeneric"><text></text><text></text></binding></visual></toast>')
$t = $xml.GetElementsByTagName('text')
$t.Item(0).AppendChild($xml.CreateTextNode($env:PTRACK_NOTIFY_TITLE)) | Out-Null
$t.Item(1).AppendChild($xml.CreateTextNode($env:PTRACK_NOTIFY_BODY)) | Out-Null
$toast = New-Object Windows.UI.Notifications.ToastNotification $xml
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe').Show($toast)
"#;

/// Called by the supervisor once the job has ended and been saved.
pub fn job_finished(job: &Job) {
    let cfg = config::load().notify;
    // Whoever stopped the job already knows it ended.
    if job.state == State::Cancelled || job.duration_ms() < cfg.min_seconds * 1000 {
        return;
    }
    let (title, body) = message(job);
    // The notifier may outlive us; it must not hold the caller's output pipe
    // open, or an agent waiting for this command would wait for it too.
    crate::platform::stop_sharing_std_handles();
    if cfg.desktop {
        if let Some(mut cmd) = desktop_command(&title, &body) {
            // Don't wait: the notifier finishes on its own after we exit.
            let _ = cmd.spawn();
        }
    }
    if let Some(url) = &cfg.webhook {
        let (content_type, payload) = webhook_payload(job, cfg.webhook_format, &title, &body);
        let _ = send_webhook(url, &content_type, &payload, false);
    }
}

/// The body to POST and its content type.
pub fn webhook_payload(
    job: &Job,
    format: WebhookFormat,
    title: &str,
    body: &str,
) -> (String, String) {
    let text = format!("{title}\n{body}");
    match format {
        WebhookFormat::Json => (
            "application/json".into(),
            json!({ "text": text, "content": text, "job": crate::commands::to_json(job) })
                .to_string(),
        ),
        WebhookFormat::Text => ("text/plain; charset=utf-8".into(), text),
    }
}

/// Only plain web addresses: anything else could be read by curl as an
/// option or a local file.
pub fn valid_webhook_url(url: &str) -> bool {
    (url.starts_with("https://") || url.starts_with("http://"))
        && !url.chars().any(char::is_whitespace)
}

fn curl_program() -> PathBuf {
    // Prefer Windows' own curl over whatever else is first on PATH.
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("SystemRoot") {
        let builtin = PathBuf::from(root).join("System32").join("curl.exe");
        if builtin.is_file() {
            return builtin;
        }
    }
    PathBuf::from("curl")
}

/// POSTs `payload` to `url`. With `wait`, reports whether it was delivered;
/// without, returns as soon as curl has the data (curl gives up after 10 s).
fn send_webhook(url: &str, content_type: &str, payload: &str, wait: bool) -> Result<(), String> {
    if !valid_webhook_url(url) {
        return Err(format!("not an http(s) address: {url}"));
    }
    let mut cmd = Command::new(curl_program());
    cmd.args(["--silent", "--show-error", "--fail", "--max-time", "10"])
        .args(["--request", "POST", "--header"])
        .arg(format!("Content-Type: {content_type}"))
        // No "Expect: 100-continue" round trip (adds a 1 s wait on many servers).
        .args(["--header", "Expect:"])
        .args(["--data-binary", "@-", "--url", url])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(if wait { Stdio::piped() } else { Stdio::null() });
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("could not run curl: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(payload.as_bytes());
        // Dropping stdin here ends the upload.
    }
    if !wait {
        return Ok(());
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("curl failed: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "webhook not delivered: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// `("Job failed: npm test", "took 3m04s · exit 1\nnpm ERR! ...")`
pub fn message(job: &Job) -> (String, String) {
    // Starts with "Job" so it can never be mistaken for a command-line option.
    let title = format!("Job {}: {}", job.state.as_str(), truncate(&job.title(), 60));
    let mut body = format!("took {}", fmt_duration(job.duration_ms()));
    if let Some(code) = job.exit_code {
        body.push_str(&format!(" · exit {code}"));
    }
    if let Some(line) = &job.last_line {
        body.push('\n');
        body.push_str(&truncate(line, 120));
    }
    (title, body)
}

/// The command that shows one desktop notification, or None where there is
/// no known way to do it.
fn desktop_command(title: &str, body: &str) -> Option<Command> {
    #[cfg(windows)]
    let mut cmd = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut c = Command::new("powershell.exe");
        c.args(["-NoProfile", "-NonInteractive", "-Command", WINDOWS_TOAST])
            .env("PTRACK_NOTIFY_TITLE", title)
            .env("PTRACK_NOTIFY_BODY", body)
            .creation_flags(CREATE_NO_WINDOW);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = Command::new("osascript");
        c.args([
            "-e",
            "on run argv",
            "-e",
            "display notification (item 2 of argv) with title (item 1 of argv)",
            "-e",
            "end run",
            title,
            body,
        ]);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let mut c = Command::new("notify-send");
        c.args(["--app-name=ptrack", "--", title, body]);
        c
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    Some(cmd)
}

const USAGE: &str = "usage: ptrack notify [on | off | min-seconds N | webhook URL | webhook off | webhook-format json|text | test]";

/// `ptrack notify ...`: show or change notification settings, or send a test.
pub fn command(args: &[String]) -> Result<i32, String> {
    let mut cfg = config::load_checked()?;
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        [] => {
            show(&cfg);
            return Ok(0);
        }
        ["on"] => cfg.notify.desktop = true,
        ["off"] => cfg.notify.desktop = false,
        ["min-seconds", n] => {
            cfg.notify.min_seconds = n
                .parse()
                .map_err(|_| format!("min-seconds needs a number, got '{n}'"))?;
        }
        ["webhook", "off"] => cfg.notify.webhook = None,
        ["webhook", url] => {
            if !valid_webhook_url(url) {
                return Err(format!(
                    "the webhook must be an http:// or https:// address, got '{url}'"
                ));
            }
            cfg.notify.webhook = Some(url.to_string());
        }
        ["webhook-format", "json"] => cfg.notify.webhook_format = WebhookFormat::Json,
        ["webhook-format", "text"] => cfg.notify.webhook_format = WebhookFormat::Text,
        ["test"] => return send_test(&cfg),
        _ => return Err(USAGE.into()),
    }
    config::save(&cfg).map_err(|e| format!("cannot save settings: {e}"))?;
    show(&cfg);
    Ok(0)
}

fn show(cfg: &Config) {
    let n = &cfg.notify;
    println!(
        "desktop notifications: {}",
        if n.desktop { "on" } else { "off" }
    );
    match &n.webhook {
        Some(url) => println!("webhook: {url} ({:?})", n.webhook_format),
        None => println!("webhook: off"),
    }
    println!("only for jobs that ran at least: {}s", n.min_seconds);
    println!("settings file: {}", config::path().display());
}

/// Sends a test desktop notification (when desktop notifications are on, or
/// nothing else is configured) and a test webhook (when one is set).
fn send_test(cfg: &Config) -> Result<i32, String> {
    let n = &cfg.notify;
    let mut job = Job::new(
        "test00".into(),
        vec!["ptrack".into(), "notify".into()],
        false,
    );
    job.label = Some("ptrack test".into());
    job.last_line = Some("Notifications work.".into());
    job.finish(State::Passed, Some(0));
    let (title, body) = message(&job);

    if n.desktop || n.webhook.is_none() {
        let mut cmd = desktop_command(&title, &body)
            .ok_or("desktop notifications are not supported on this system")?;
        let status = cmd.status().map_err(|e| {
            let hint = if cfg!(all(unix, not(target_os = "macos"))) {
                " (on Ubuntu/Debian: sudo apt install libnotify-bin)"
            } else {
                ""
            };
            format!("could not run the notifier: {e}{hint}")
        })?;
        if !status.success() {
            return Err(format!("the notifier failed ({status})"));
        }
        println!("sent a test desktop notification; you should see it now");
    }
    if let Some(url) = &n.webhook {
        let (content_type, payload) = webhook_payload(&job, n.webhook_format, &title, &body);
        send_webhook(url, &content_type, &payload, true)?;
        println!("delivered a test message to {url}");
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_has_state_title_exit_and_last_line() {
        let mut job = Job::new("abc123".into(), vec!["make".into()], false);
        job.label = Some("build".into());
        job.last_line = Some("error: boom".into());
        job.finish(State::Failed, Some(2));
        let (title, body) = message(&job);
        assert_eq!(title, "Job failed: build");
        assert!(body.starts_with("took "), "{body}");
        assert!(
            body.contains("exit 2") && body.ends_with("error: boom"),
            "{body}"
        );
    }
}
