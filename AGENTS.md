# AGENTS.md: rules for AI coding agents working on this repository

Read this whole file before changing anything. If a task's instructions conflict with this file,
this file wins. Stop and ask the owner if you're unsure.

## What this project is

`ptrack` is a single small binary, written in Rust, that runs commands as tracked "jobs". It shows
what is running and lets people stop, pause or resume it. It is built for AI coding agents and
the humans watching them. Windows, Linux and macOS are all first-class.

Key idea: **one JSON file per job** (`<home>/jobs/<id>.json`) plus one log file
(`<home>/logs/<id>.log`). There is no database and no daemon.

## Map of the code

| File | Responsibility |
|---|---|
| `src/main.rs` | Argument parsing (hand-written `Parser`), `HELP` text, dispatch to commands |
| `src/supervise.rs` | `ptrack run`: starts the command, pumps output to log/terminal, heartbeat, stop handling, final state. `detach()` starts a background copy of itself |
| `src/job.rs` | The `Job` record, its file paths, atomic save/load, `effective_state()` (running/paused/lost logic), `resolve()` (id prefix lookup) |
| `src/commands.rs` | `list`, `status`, `tail`, `wait`, `stop`, `pause`, `resume`, `prune`, plus shared helpers `to_json`, `visible_jobs`, `stop_job`, `pause_job`, `resume_job` |
| `src/platform/windows.rs` | Job Objects, Ctrl+Break, suspend/resume, detached spawn, PATHEXT lookup |
| `src/platform/unix.rs` | Process groups, signals, detached spawn (setsid) |
| `src/web.rs` + `src/ui.html` | `ptrack ui`: localhost HTTP server (std only) and the one-page dashboard |
| `src/mcp.rs` | `ptrack mcp`: MCP server over stdio (JSON-RPC, hand-written) |
| `src/hook.rs` | `ptrack hook ...`: Claude Code hook handler, plus install/uninstall into settings.json |
| `src/util.rs` | Time formatting, ids, tokens, text cleanup, `last_lines` |
| `tests/cli.rs` | End-to-end tests that run the real binary with a private `PTRACK_HOME` |

## Rules you must follow

1. **Single writer.** Only the job's own supervisor (in `supervise.rs`) writes `jobs/<id>.json`.
   Other commands only read it. To ask the supervisor for something, they write a small marker
   file (`<id>.stop`, `<id>.paused`).
2. **No new dependencies.** The only crates allowed are `serde`, `serde_json`, `libc` (Unix) and
   `windows-sys` (Windows; you may enable more of its features). Use the standard library for
   everything else. If you think you need a crate, stop and ask.
3. **Windows is first-class.** Every change must compile and pass lint for Windows, Linux and
   macOS. `scripts/verify.*` checks all three from any machine.
4. **Never weaken security.**
   - The web server binds only `127.0.0.1`, and every `/api/` call needs the token and a
     localhost `Host` header. Keep both checks.
   - Hooks never approve commands (no `permissionDecision`).
   - Never pass `-ExecutionPolicy Bypass` or similar flags to PowerShell.
   - Pass user-controlled text (job titles, output lines) to other programs through arguments or
     environment variables, never by pasting it into script source.
5. **Quiet channels.**
   - `ptrack mcp` prints only JSON-RPC on stdout.
   - `ptrack hook claude-code` prints only its JSON reply, or nothing.
   - The hook must exit 0 even on errors.
6. **Compatibility of job files.** New fields on `Job` must be `Option<...>` with `#[serde(default)]`
   so older job files still load.
7. **Every user-visible feature needs:** CLI wiring in `main.rs`, a line in `HELP`, a README
   section, and at least one test in `tests/cli.rs` (or a unit test).
8. **Don't disable, skip or loosen existing tests** to make a change pass. If a test must change
   because behaviour intentionally changed, say so in the commit message.
9. **Match the surrounding style:** small functions, comments that explain *why*, no `unwrap()`
   on anything that comes from the user or the file system (tests may unwrap).

## How to check your work

Run this after every change, from the repository root:

```sh
sh scripts/verify.sh       # Git Bash, Linux or macOS
./scripts/verify.ps1       # PowerShell on Windows
```

If PowerShell refuses to run the script ("running scripts is disabled on this system"), use
`sh scripts/verify.sh` from Git Bash instead. **Never** bypass the execution policy.

It must end with `ALL CHECKS PASSED`. It runs: `cargo fmt --check`, `cargo clippy` for this
machine plus the two other systems, and `cargo test`. If formatting fails, run `cargo fmt` and
re-run. Unix-only code (`#[cfg(unix)]`) cannot be *run* on Windows. It is only lint-checked there
and runs in GitHub CI after you push.

Try the binary by hand with a throwaway data folder, so you don't touch the owner's real jobs:

```sh
PTRACK_HOME=/tmp/ptrack-try ./target/debug/ptrack run -- echo hi        # Unix shells
$env:PTRACK_HOME="$env:TEMP\ptrack-try"; .\target\debug\ptrack.exe list  # PowerShell
```

The owner has the ptrack Claude Code hook installed. If you are Claude Code, long commands such as
`cargo test` are wrapped by the **installed** ptrack, and their output starts and ends with a
`[ptrack] job …` line. That is expected and harmless. Ignore those lines when reading results.

Windows gotcha: Windows can't overwrite a running `.exe`. If a background job is still running from
`target/debug/ptrack.exe`, `cargo build` fails with "Access denied". Stop the job first
(`ptrack stop <id> --force`).

## Git

- Work directly on `main`. **Do not create branches** (owner's rule).
- Commit once per finished phase, after `verify` passes. Use the commit message given in the plan.
  End every commit message with the attribution line your tool normally uses.
- Push after each commit (`git push origin main`), so CI runs the tests on Linux and macOS. Then
  check the result with `gh run list --limit 1` / `gh run watch <id>`. A phase is only done when
  CI is green.
- Never push tags or create releases. The owner does that.

## Work plan

Planned features and step-by-step instructions are in [`docs/plan/`](docs/plan/README.md).
