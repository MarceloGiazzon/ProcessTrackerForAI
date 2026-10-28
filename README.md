# ProcessTrackerForAI (`ptrack`)

See and control the long-running processes your AI coding agents start, for free:
**what is running, who started it, since when, is it alive, how did it end**, and stop, pause or
resume it.

An agent starts a build, a test suite or a server. You ask "status?" and the agent spends tokens
reading logs to find out. `ptrack` records every job in one place, so you can look at a list (zero
tokens) and the agent can make one short call instead of reading a log.

- **One small file, nothing to install.** Under 1 MB. No runtime, no database, no background service.
- **Windows is a first-class platform**, along with Linux and macOS.
- **Works with any agent** (Claude Code, Codex, Cursor, Gemini CLI, Aider, plain scripts) and any project.
- **Honest states:** `running`, `paused`, `passed`, `failed`, `cancelled`, `lost`. A job whose
  supervisor vanished is shown as `lost`, never as still running and never as passed.
- **Stops the whole tree:** everything the command started, not just the first process.
- **Local only.** No account, no network, no telemetry.

- **A live dashboard in your browser** (`ptrack ui`) and an **MCP server** for agents (`ptrack mcp`).

Status: v0.2, early. Feedback welcome.

## Install

**Linux and macOS:** open a terminal and run:

```sh
curl -fsSL https://raw.githubusercontent.com/MarceloGiazzon/ProcessTrackerForAI/main/install.sh | sh
```

This installs into `~/.local/bin`. If that folder isn't on your `PATH` yet, the script prints the
one line that adds it.

**Windows:** open PowerShell and run:

```powershell
irm https://raw.githubusercontent.com/MarceloGiazzon/ProcessTrackerForAI/main/install.ps1 | iex
```

This installs into `%LOCALAPPDATA%\Programs\ptrack` and adds that folder to your `PATH`. Open a new
terminal afterwards.

Both scripts download the latest release from GitHub and check its checksum. Run the same command
again to upgrade. Or download the file for your system from the Releases page yourself:

| System | File |
|---|---|
| Windows (most PCs) | `ptrack-…-x86_64-pc-windows-msvc.zip` |
| Windows on ARM | `ptrack-…-aarch64-pc-windows-msvc.zip` |
| Linux | `ptrack-…-x86_64-unknown-linux-musl.tar.gz` (or `aarch64` for ARM) |
| Mac with Apple chip (M1 and later) | `ptrack-…-aarch64-apple-darwin.tar.gz` |
| Older Intel Mac | `ptrack-…-x86_64-apple-darwin.tar.gz` |

Or build it from source if you have Rust: `cargo install --path .`

Check that it works: `ptrack version`

## Quick start

```sh
# Run a command as a tracked job (output still shows in your terminal)
ptrack run -- npm test

# Start something long in the background and get your prompt back
ptrack run -d -l "full build" -- gradle build
#   job k3f9a2 started in background

ptrack list                 # what is running and what ran recently
ptrack status k3f9a2        # details of one job
ptrack tail k3f9a2 -f       # watch its output
ptrack wait k3f9a2          # wait for it; exits with the job's own exit code
ptrack pause k3f9a2         # freeze it (ptrack resume k3f9a2 to continue)
ptrack stop k3f9a2          # stop it and everything it started
ptrack rerun k3f9a2         # run the same command again (in the same folder)
```

`ptrack list` looks like this:

```
ID      STATE         TIME   STARTED  AGENT        WHAT                     LAST LINE
k3f9a2  running      3m04s  3m04s ago claude-code  full build               > Task :app:compileJava
x9ewes  passed         41s    12m ago claude-code  npm test                 Tests: 212 passed, 212 total
cexj44  failed          2s     1h ago -            ./deploy.sh              error: missing API_URL
```

## Tell your AI agent to use it

Paste this into your agent's instructions file (`CLAUDE.md`, `AGENTS.md`, `.cursorrules`, …):

```markdown
## Long-running commands
Run any command that may take more than ~30 seconds (builds, test suites, servers, downloads,
migrations) through `ptrack`:
- In the background: `ptrack run -d -l "<short label>" -- <command>`. It prints a job id.
- To check progress cheaply: `ptrack status <id>` or `ptrack list`. They show the state and the
  last output line, so there is no need to read the log.
- To wait for the result: `ptrack wait <id> --timeout 600`. It exits with the job's exit code and
  prints the last 20 lines only if the job failed.
- In the foreground with little output: `ptrack run -q -- <command>`. It prints one summary line,
  plus the last lines on failure.
- To stop a job: `ptrack stop <id>`.
```

## Dashboard

```sh
ptrack ui
```

This opens a page in your browser with every job, updated live. Click a job to see its output. The
buttons pause, resume or stop it. The page is reachable only from your own computer and is
protected by a random key in the link, which changes every time you start `ptrack ui`. Press
Ctrl+C in the terminal to close it. Options: `--port N` and `--no-open`.

Tick **Notify when jobs end** to get a notification from your browser whenever a job finishes.
This works only while the page is open. The first time, the browser asks for permission. The
**Rerun** button on a finished job starts its command again, in the same folder.

## Notifications when a job ends

```sh
ptrack notify on        # desktop notification when a job ends (off by default)
ptrack notify test      # check that notifications reach your screen
ptrack notify           # show the current settings
ptrack notify off
```

Only jobs that ran at least 30 seconds trigger a notification, so quick commands don't spam you.
Change that with `ptrack notify min-seconds N`. Jobs you stopped yourself don't notify. ptrack uses
what each system already has: PowerShell on Windows, where the notification says it comes from
"Windows PowerShell"; `osascript` on macOS; and `notify-send` on Linux. If `notify-send` is
missing on Ubuntu, install it with `sudo apt install libnotify-bin`.

To get a message on your phone or in a chat when a job ends, add a **webhook**. ptrack then POSTs
every finished job to that address. It uses `curl`, which Windows 10+, macOS and Linux already
have:

```sh
ptrack notify webhook https://hooks.slack.com/services/...   # Slack or Discord: works as is
ptrack notify webhook https://ntfy.sh/my-secret-topic        # ntfy phone app: also run
ptrack notify webhook-format text                            #   (plain text instead of JSON)
ptrack notify test                                           # send a test message now
ptrack notify webhook off
```

The JSON payload has `text` (read by Slack), `content` (read by Discord) and `job` (the full job
record, for your own scripts). Webhook messages include the command line and its last output line,
so only send them to a service you trust.

## MCP server (for agents)

`ptrack mcp` lets MCP-capable agents ask about jobs with one small tool call. Tools:
`jobs_list`, `job_status`, `job_tail`, `job_wait`. It is **read-only by default**. Add
`--allow-control` to also allow `job_stop`, `job_pause` and `job_resume`. Starting jobs stays in
the shell (`ptrack run -d ...`) on purpose.

Claude Code:

```sh
claude mcp add --scope user ptrack -- ptrack mcp
```

Other MCP clients (Cursor, Gemini CLI, …) use the usual JSON config:

```json
{ "mcpServers": { "ptrack": { "command": "ptrack", "args": ["mcp"] } } }
```

## Automatic tracking in Claude Code (hook)

With the hook, you don't have to rely on the agent remembering to use ptrack:

```sh
ptrack hook install            # for all your projects (~/.claude/settings.json)
ptrack hook install --project  # only this project (.claude/settings.local.json)
```

Restart Claude Code afterwards. From then on:

- **Long-running commands** (builds, tests, installs, servers, Docker, and similar) and **every
  background command** run through ptrack automatically. Claude sees the same output and exit code
  as before, plus one `[ptrack]` line at the start and one at the end.
- Each job is labelled with the description Claude gave the command, and linked to the **session
  and the prompt** that caused it. A 200-character preview of each prompt is kept locally. Use
  `--no-prompts` if you don't want that.
- `--all` wraps every command, not just long-running ones.
- Undo with `ptrack hook uninstall` (add `--project` if you installed it that way). Your other
  settings are kept, and a backup is written next to the settings file.

**Permission rules:** the hook never approves anything. Claude Code checks its permission rules
against the *rewritten* command, so an allow rule such as `Bash(npm test:*)` no longer matches a
wrapped `npm test`. In the default permission mode, Claude Code will ask you before running it.
Auto mode and bypass mode are not affected.

## Who started each job

ptrack works out which agent and session started a job by itself when the agent provides that
information (`AI_AGENT`, `CLAUDECODE`, `CLAUDE_CODE_SESSION_ID`). For any other agent, use
`--agent` / `--session`, or set the `PTRACK_AGENT` / `PTRACK_SESSION` environment variables.

## Commands

| Command | What it does |
|---|---|
| `ptrack run [options] -- <command>` | Runs a command as a tracked job. Its exit code is the command's own. |
| `ptrack list [--all] [--here] [--json]` | Running jobs plus recent ones. `--here`: only jobs started in this folder. |
| `ptrack status [ID] [--json]` | Everything about one job. No ID means the newest job. |
| `ptrack tail [ID] [-n LINES] [-f]` | The job's output. `-f` keeps following it until the job ends. |
| `ptrack wait [ID] [--timeout SECS] [-n LINES]` | Blocks until the job ends. Exits with its code, or 124 on timeout. |
| `ptrack stop ID [--grace SECS] [--force]` | Asks the job to exit, then force-stops the whole tree after the grace time (default 5 s). |
| `ptrack pause ID`, `ptrack resume ID` | Freezes and unfreezes every process of the job. |
| `ptrack rerun ID [-d] [-q]` | Runs a finished job's command again, in the same folder, as a new job linked to the old one. |
| `ptrack prune [--days N] [--all]` | Deletes finished jobs and their logs (default: older than 7 days). |
| `ptrack ui [--port N] [--no-open]` | Live dashboard in your browser (this computer only). |
| `ptrack mcp [--allow-control]` | MCP server over stdio for AI agents. |
| `ptrack hook install [--project] [--all] [--no-prompts]` | Claude Code hook: long commands run through ptrack automatically. |
| `ptrack hook uninstall [--project]` | Removes the hook. |
| `ptrack notify [on\|off\|min-seconds N\|test]` | Desktop notification when a job ends. |
| `ptrack notify webhook URL\|off`, `ptrack notify webhook-format json\|text` | Also POST each finished job to a web address. |

Options for `run`: `-l/--label TEXT`, `-d/--detach` (background), `-q/--quiet` (summary only),
`-s/--shell` (run the text through `cmd.exe` or `sh`, for `&&`, pipes, etc.), `--agent`, `--session`.

`ID` can be the first few characters of a job id, or `last`.

## How it works

`ptrack run` becomes the **supervisor** of the command. It starts the command, copies its output to a
log file, refreshes the job's record every 2 seconds (the *heartbeat*), and writes the final result
when the command ends. With `-d`, a background copy of ptrack does the same job.

Everything lives in plain files in a per-user folder:

| System | Folder |
|---|---|
| Windows | `%LOCALAPPDATA%\ptrack` |
| Linux | `~/.local/share/ptrack` (or `$XDG_DATA_HOME/ptrack`) |
| macOS | `~/Library/Application Support/ptrack` |

Set `PTRACK_HOME` to use another folder. Inside it, `jobs/<id>.json` is the job record (readable by
people and agents) and `logs/<id>.log` is the output. Each job file is written only by its own
supervisor. Other commands only read it, or leave small request files next to it (`<id>.stop`,
`<id>.paused`). So there is no database and no locking.

If a job file says "running" but its heartbeat is older than 10 seconds and its supervisor process
no longer exists, the job is shown as **lost**.

### Process control per system

| | Windows | Linux / macOS |
|---|---|---|
| Group the whole tree | A named Job Object. The command starts frozen, joins the job, then runs, so no child can escape. | Its own process group |
| Stop, polite first | Ctrl+Break | SIGTERM |
| Stop, forced | Terminate the Job Object | SIGKILL to the group |
| Pause / resume | Suspend / resume every process in the job | SIGSTOP / SIGCONT to the group |
| If the supervisor itself is killed | The tree is killed with it, so nothing is left running unrecorded | The tree may keep running (see limits below) |

## Honest limits

- **Pause is not always safe.** While frozen, network connections and timers inside the job can time
  out, and some servers or JVM-based tools (Gradle, Maven) may not recover cleanly. Prefer stop and
  rerun for those.
- **Polite stop on Windows:** many console programs exit on Ctrl+Break, but some don't. Java prints
  a thread dump; `ping` prints statistics. ptrack then force-stops them when the grace time is up.
- **On Linux/macOS, a killed supervisor can leave the job running.** It is then shown as `lost`, and
  `ptrack stop` tells you the process group to kill by hand. Windows doesn't have this problem.
- **Background helpers are left alone after a normal exit.** For example, a Gradle daemon started by
  a build keeps running, just as it would without ptrack. `stop` does kill them.
- **No input.** Jobs get no keyboard input, so commands that ask questions get an end-of-input
  instead of waiting forever.
- **Secrets:** command lines and output are stored as-is. Don't pass secrets as command arguments.
  Use environment variables, which ptrack never records.
- **Text encoding on Windows:** programs that print in a legacy code page (for example `ping` on a
  non-English Windows) may show garbled accented letters in `tail` and `last line`. The log file
  itself keeps the original bytes.

## Roadmap

- ✅ v0.2: local dashboard (`ptrack ui`) and MCP server (`ptrack mcp`).
- ✅ v0.3 (in progress): Claude Code hook with prompt attribution.
- Next: rerun, desktop notifications when a job ends, no orphaned processes on Linux/macOS,
  recipes for more agents.
- v1.0: stable record format, signed binaries, winget / Scoop / Homebrew packages.

## Development

```sh
cargo test                       # unit + end-to-end tests (they run the real binary)
cargo clippy --all-targets
cargo build --release            # target/release/ptrack(.exe)
```

Dependencies are kept minimal on purpose: `serde`/`serde_json`, plus `windows-sys` (Windows) or
`libc` (Unix). Releases are built by `.github/workflows/release.yml` when a `v*` tag is pushed.

## License

MIT
