//! `ptrack ui`: a local web page showing all jobs live, with stop / pause /
//! resume buttons. Standard library only: a tiny HTTP/1.1 server bound to
//! 127.0.0.1 that serves one embedded HTML page and a small JSON API.
//!
//! Safety: only loopback connections; every API call must carry a random
//! per-run token (sent in a custom header, which also blocks cross-site
//! requests from web pages); and the Host header must be localhost, which
//! blocks DNS-rebinding attacks.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde_json::json;

use crate::commands::{self, Stopped};
use crate::job;
use crate::supervise::summary;
use crate::util::{last_lines, random_token, url_decode};

const PAGE: &str = include_str!("ui.html");
const DEFAULT_PORT: u16 = 7381;

pub fn serve(port: Option<u16>, open: bool) -> i32 {
    let listener = match port {
        Some(p) => TcpListener::bind(("127.0.0.1", p)),
        None => TcpListener::bind(("127.0.0.1", DEFAULT_PORT))
            .or_else(|_| TcpListener::bind(("127.0.0.1", 0))),
    };
    let listener = match listener {
        Ok(l) => l,
        Err(e) => {
            eprintln!("ptrack: cannot open a local port: {e}");
            return 1;
        }
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    let token = Arc::new(random_token());
    // The token travels in the URL fragment, which browsers never send to servers or logs.
    let url = format!("http://127.0.0.1:{port}/#t={token}");
    println!("ptrack dashboard: {url}");
    println!("(only reachable from this computer; press Ctrl+C to stop)");
    if open {
        open_browser(&url);
    }

    let allowed_hosts = Arc::new([format!("127.0.0.1:{port}"), format!("localhost:{port}")]);
    for stream in listener.incoming().flatten() {
        let token = token.clone();
        let hosts = allowed_hosts.clone();
        thread::spawn(move || {
            let _ = handle(stream, &token, &hosts[..]);
        });
    }
    0
}

struct Request {
    method: String,
    path: String,
    query: String,
    host: String,
    token: String,
}

fn read_request(stream: &TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut first = String::new();
    reader.read_line(&mut first).ok()?;
    let mut parts = first.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target, String::new()),
    };
    let (mut host, mut token) = (String::new(), String::new());
    let mut header_bytes = 0;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).ok()?;
        header_bytes += n;
        if n == 0 || line == "\r\n" || line == "\n" || header_bytes > 16 * 1024 {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            match k.trim().to_ascii_lowercase().as_str() {
                "host" => host = v.trim().to_string(),
                "x-ptrack-token" => token = v.trim().to_string(),
                _ => {}
            }
        }
    }
    Some(Request {
        method,
        path,
        query,
        host,
        token,
    })
}

fn handle(mut stream: TcpStream, token: &str, hosts: &[String]) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let Some(req) = read_request(&stream) else {
        return Ok(());
    };
    if !hosts.contains(&req.host) {
        return respond(&mut stream, 403, "text/plain", "forbidden host");
    }
    if req.method == "GET" && (req.path == "/" || req.path == "/index.html") {
        return respond(&mut stream, 200, "text/html; charset=utf-8", PAGE);
    }
    if !req.path.starts_with("/api/") {
        return respond(&mut stream, 404, "text/plain", "not found");
    }
    if !constant_time_eq(req.token.as_bytes(), token.as_bytes()) {
        return respond(&mut stream, 401, "text/plain", "missing or wrong token");
    }

    let segments: Vec<&str> = req.path.trim_start_matches("/api/").split('/').collect();
    match (req.method.as_str(), segments.as_slice()) {
        ("GET", ["jobs"]) => {
            let all = query_param(&req.query, "all").is_some_and(|v| v == "1");
            let param = |k| {
                query_param(&req.query, k)
                    .map(url_decode)
                    .filter(|v| !v.is_empty())
            };
            let (project, tag) = (param("project"), param("tag"));
            let filter = commands::Filter {
                cwd_prefix: None,
                project: project.as_deref(),
                tag: tag.as_deref(),
            };
            let (jobs, total) = commands::visible_jobs(all, &filter);
            let (projects, tags) = projects_and_tags();
            let body = json!({
                "jobs": jobs.iter().map(commands::to_json).collect::<Vec<_>>(),
                "total": total,
                "projects": projects,
                "tags": tags,
                "home": job::home().display().to_string(),
            });
            respond(&mut stream, 200, "application/json", &body.to_string())
        }
        ("GET", ["jobs", id, "log"]) => match job::resolve(Some(id)) {
            Ok(j) => {
                let n = query_param(&req.query, "lines")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(200usize)
                    .min(5000);
                let lines = last_lines(Path::new(&j.log_path), n).unwrap_or_default();
                respond(
                    &mut stream,
                    200,
                    "text/plain; charset=utf-8",
                    &lines.join("\n"),
                )
            }
            Err(e) => respond(&mut stream, 404, "text/plain", &e),
        },
        ("POST", ["jobs", id, action]) => {
            let result = job::resolve(Some(id)).and_then(|j| match *action {
                "stop" => commands::stop_job(&j, 5_000).map(|s| match s {
                    Stopped::Already(d) => {
                        format!("job {} had already {}", d.id, d.effective_state().as_str())
                    }
                    Stopped::Now(d) => summary(&d),
                }),
                "pause" => commands::pause_job(&j).map(|_| format!("paused job {}", j.id)),
                "resume" => commands::resume_job(&j).map(|_| format!("resumed job {}", j.id)),
                "rerun" => commands::rerun_detached(&j),
                other => Err(format!("unknown action '{other}'")),
            });
            let (code, body) = match result {
                Ok(msg) => (200, json!({ "ok": true, "message": msg })),
                Err(msg) => (409, json!({ "ok": false, "message": msg })),
            };
            respond(&mut stream, code, "application/json", &body.to_string())
        }
        _ => respond(&mut stream, 404, "text/plain", "not found"),
    }
}

/// Every project and tag among all jobs, sorted, for the dashboard's filters.
fn projects_and_tags() -> (Vec<String>, Vec<String>) {
    let mut projects = std::collections::BTreeSet::new();
    let mut tags = std::collections::BTreeSet::new();
    for j in job::list_all() {
        projects.extend(j.project_name());
        tags.extend(j.tag);
    }
    (projects.into_iter().collect(), tags.into_iter().collect())
}

fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn respond(
    stream: &mut TcpStream,
    code: u16,
    content_type: &str,
    body: &str,
) -> std::io::Result<()> {
    let reason = match code {
        200 => "OK",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        _ => "Error",
    };
    write!(
        stream,
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\n\
         Referrer-Policy: no-referrer\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    stream.flush()
}

fn open_browser(url: &str) {
    use std::process::{Command, Stdio};
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler").arg(url);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    let _ = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}
