# ProcessTrackerForAI (`ptrack`)

See and control the long-running processes your AI coding agents start, for free:
**what is running, who started it, since when, is it alive, how did it end**, and stop, pause or
resume it.

An agent starts a build, a test suite or a server. You ask "status?" and the agent spends tokens
reading logs to find out. `ptrack` records every job in one place, so you can look at a list (zero
tokens) and the agent can make one short call instead of reading a log.

- **One small file, nothing to install.** Under 3 MB. No runtime, no database, no background service.
- **Windows is a first-class platform**, along with Linux and macOS.
- **Works with any agent** (Claude Code, Codex, Cursor, Gemini CLI, Aider, plain scripts) and any project.
- **Honest states:** `running`, `paused`, `passed`, `failed`, `cancelled`, `lost`. A job whose
  supervisor vanished is shown as `lost`, never as still running and never as passed.
- **Stops the whole tree:** everything the command started, not just the first process.
- **Local only.** No account, no network, no telemetry.

- **A live dashboard in your browser** (`ptrack ui`) and an **MCP server** for agents (`ptrack mcp`).

Status: v0.6, early. Feedback welcome.

A step-by-step manual for non-specialists, in Portuguese, is in [docs/manual.html](docs/manual.html)
(download it and open it in your browser).

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
ID      STATE         TIME   STARTED  PROJECT              TAG          AGENT        WHAT                           LAST LINE
k3f9a2  running      3m04s  3m04s ago NPDev                CI Watch     claude-code  full build                     > Task :app:compileJava
x9ewes  passed         41s    12m ago NPDev                -            claude-code  npm test                       Tests: 212 passed, 212 total
cexj44  failed          2s     1h ago website              -            -            ./deploy.sh                    error: missing API_URL
```

Run commands from inside your project's folder: `npm test` needs the folder with
`package.json`, and `gradle build` needs Gradle installed. ptrack runs the command exactly as you
would, so it fails the same way when it would fail without ptrack.

## Projects and tags

Every job belongs to a **project**, shown in the `PROJECT` column. ptrack works it out by itself:
it is the name of the git repository the command ran in (so a job started in `NPDev/src/app`
belongs to `NPDev`). Outside a repository it is the name of the current folder. Set it yourself
with `--project NAME` or the `PTRACK_PROJECT` environment variable.

A **tag** is an optional free-form word or phrase to group jobs across projects, such as
`CI Watch`, `nightly` or `release`. Set it with `--tag TAG` or `PTRACK_TAG`.

```sh
ptrack run -d --tag "CI Watch" -- gh run watch 1234   # project: worked out from the folder
ptrack list --project NPDev                           # only that project's jobs
ptrack list --tag "ci watch"                          # only jobs with that tag (any case)
```

`ptrack rerun` keeps the project and the tag. The dashboard shows both on every job, and the MCP
tool `jobs_list` accepts `project` and `tag` filters.

## Tell your AI agent to use it

Add this text to your agent's instructions file (`CLAUDE.md`, `AGENTS.md`, `GEMINI.md`,
`CONVENTIONS.md`, …). `ptrack instructions` prints it, so you can append it in one go:

```sh
ptrack instructions >> AGENTS.md                 # Git Bash, Linux, macOS
ptrack instructions | Add-Content AGENTS.md      # PowerShell
```

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
- Add `--tag "<tag>"` to group related jobs; `ptrack list --tag "<tag>"` shows only those.
- After starting a job, end your reply with one line telling the user how to follow it, e.g.
  `ptrack job <id>: follow with "ptrack tail <id> -f", or open "ptrack ui --job <id>".`
```

**Step-by-step setup for Codex, Cursor, Antigravity CLI / Gemini CLI and Aider** (instructions file,
MCP and attribution for each): see [docs/agents.md](docs/agents.md).

## Dashboard

```sh
ptrack ui
```

This opens a page in your browser with every job, updated live. Click a job to see its output. The
buttons pause, resume or stop it. The page is reachable only from your own computer and is
protected by a random key in the link, which changes every time you start `ptrack ui`. Press
Ctrl+C in the terminal to close it. Options: `--port N`, `--no-open`, and `--job ID` to open the
page on one job (it then shows every project and tag, so that job is in view). If a dashboard is
already running, `ptrack ui` opens that one instead of starting a second.

While the dashboard runs, its address and key are kept in `ui.json` in your data folder (readable
only by you). That is how the Claude Code hook can give you a clickable link to each new job.

Every job tells you how to follow it. ptrack's first line for a job reads
`[ptrack] job k3f9a2 started · watch it live: ptrack tail k3f9a2 -f · in your browser: ptrack ui --job k3f9a2`,
and the agent instructions and the MCP server ask agents to end their reply with the same hint.

Tick **Notify when jobs end** to get a notification from your browser whenever a job finishes.
This works only while the page is open. The first time, the browser asks for permission. The
**Rerun** button on a finished job starts its command again, in the same folder. The **project**
and **tag** menus at the top show only the jobs of one project or tag; the page remembers your
choice.

You can open the output of several jobs at the same time. **Expand all** opens every job on screen
(at most 20 at once, so the page stays light) and **Collapse all** closes them. Each job also has a checkbox, and
**Select all** at the top ticks or clears every job on screen. **Copy selected** puts the details
and output of the ticked jobs on your clipboard (as if you had pressed Ctrl+C), and **Download
selected** merges them into one `.txt` file. The menu next to them chooses how much of each log to
take: the **full log** (the default; for logs over 32 MB only the last 32 MB, and the file says so)
or the **last 5000 lines**. A second menu chooses the format:

| Format | File | Good for |
|---|---|---|
| Text (default) | `.txt` | Reading, or pasting into a chat |
| Markdown | `.md` | An issue or pull request: one heading per job, the output in a code block |
| JSON | `.json` | Scripts, or handing the results to an AI agent: one object per job with its details and output |

Each job comes with its id, title, state, folder, start time and log path, followed by its output.

The **search box** at the top (press `/` to jump to it, `Esc` to clear it) narrows the list as you
type. Every word you type must appear in the job's id, command or label, project, tag, folder, agent,
state or last output line, in any case. In logs that are open, the matches are highlighted (up to 500
per log) and a count is shown. It does not look inside logs that are closed. Select all, Expand all,
Copy, Download and the buttons below all work on the jobs the search leaves on screen. Jobs you
ticked and then hid with a search stay ticked, but are not touched until they are shown again.

To work on several jobs at once, tick them: **Pause**, **Resume**, **Stop** and **Rerun** buttons
appear, each showing how many of the ticked jobs it applies to (for example `Stop (3)`). Jobs an
action doesn't fit, such as pausing a finished job, are skipped and the message says how many. Stop
and Rerun ask for confirmation first. Only jobs you can see are affected: ticked jobs hidden by a
filter are left alone. After Rerun the ticks are cleared, so a second click doesn't start everything
again.

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
`jobs_list` (filters: `project`, `tag`, `cwd`, `all`), `job_status`, `job_tail`, `job_wait`. It is **read-only by default**. Add
`--allow-control` to also allow `job_stop`, `job_pause`, `job_resume` and `job_rerun` (which runs
a finished job's command again). Starting new commands stays in the shell (`ptrack run -d ...`)
on purpose.

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
- **You see how to follow each job** as soon as it starts. Claude Code shows a message from the
  hook itself, so it appears even if Claude doesn't mention the job. With the dashboard open
  (`ptrack ui`), it is a link you can click:

  > This command runs as ptrack job k3f9a2. Watch it in your browser:
  > http://127.0.0.1:7381/#t=…&job=k3f9a2 (or run "ptrack tail k3f9a2 -f" in a terminal).

  Without the dashboard, it names the commands instead:

  > This command runs as ptrack job k3f9a2. To watch it live, run "ptrack tail k3f9a2 -f" in a
  > terminal, or open it in your browser with "ptrack ui --job k3f9a2".

  The link carries the dashboard's key, so it goes only to you, never into the command output
  that Claude reads.
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

ptrack works out which agent started a job from the variables agents set for the commands they
run: `AI_AGENT`, then Claude Code (`CLAUDECODE`), Codex (`CODEX_THREAD_ID`, `CODEX_SANDBOX`,
`CODEX_CI`), Cursor (`CURSOR_AGENT`), Antigravity CLI (`ANTIGRAVITY_AGENT`) and Gemini CLI
(`GEMINI_CLI`). The session comes from `CLAUDE_CODE_SESSION_ID` or `CODEX_THREAD_ID`. For any other
agent (Aider, for example), use `--agent` / `--session`, or set the `PTRACK_AGENT` /
`PTRACK_SESSION` environment variables. These two always win.

## Commands

| Command | What it does |
|---|---|
| `ptrack run [options] -- <command>` | Runs a command as a tracked job. Its exit code is the command's own. |
| `ptrack list [--all] [--here] [--project NAME] [--tag TAG] [--json]` | Running jobs plus recent ones. `--here`: only jobs started in this folder. `--project`, `--tag`: only those jobs. |
| `ptrack status [ID] [--json]` | Everything about one job. No ID means the newest job. |
| `ptrack tail [ID] [-n LINES] [-f]` | The job's output. `-f` keeps following it until the job ends. |
| `ptrack wait [ID] [--timeout SECS] [-n LINES]` | Blocks until the job ends. Exits with its code, or 124 on timeout. |
| `ptrack stop ID [--grace SECS] [--force]` | Asks the job to exit, then force-stops the whole tree after the grace time (default 5 s). |
| `ptrack pause ID`, `ptrack resume ID` | Freezes and unfreezes every process of the job. |
| `ptrack rerun ID [-d] [-q]` | Runs a finished job's command again, in the same folder, as a new job linked to the old one. |
| `ptrack prune [--days N] [--all]` | Deletes finished jobs and their logs (default: older than 7 days). |
| `ptrack ui [--port N] [--no-open] [--job ID]` | Live dashboard in your browser (this computer only); `--job` opens it on one job. |
| `ptrack mcp [--allow-control]` | MCP server over stdio for AI agents. |
| `ptrack hook install [--project] [--all] [--no-prompts]` | Claude Code hook: long commands run through ptrack automatically. |
| `ptrack hook uninstall [--project]` | Removes the hook. |
| `ptrack notify [on\|off\|min-seconds N\|test]` | Desktop notification when a job ends. |
| `ptrack notify webhook URL\|off`, `ptrack notify webhook-format json\|text` | Also POST each finished job to a web address. |
| `ptrack instructions` | Prints the text that tells an AI agent to use ptrack, for its instructions file. |

Options for `run`: `-l/--label TEXT`, `-d/--detach` (background), `-q/--quiet` (summary only),
`-s/--shell` (run the text through `cmd.exe` or `sh`, for `&&`, pipes, etc.), `--project NAME`,
`--tag TAG`, `--agent`, `--session`.

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
| If the supervisor itself is killed | The Job Object takes the tree down with it | A small guard process (`ptrack __guard`) notices and stops the group: SIGTERM, then SIGKILL after 5 s |

## Honest limits

- **Pause is not always safe.** While frozen, network connections and timers inside the job can time
  out, and some servers or JVM-based tools (Gradle, Maven) may not recover cleanly. Prefer stop and
  rerun for those.
- **Polite stop on Windows:** many console programs exit on Ctrl+Break, but some don't. Java prints
  a thread dump; `ping` prints statistics. ptrack then force-stops them when the grace time is up.
- **Programs that deliberately detach themselves can escape.** On Linux/macOS, a process that
  starts its own session (`setsid`, like a system daemon) leaves the job's process group, so
  stop and pause no longer reach it. The same is true on Windows for a process that breaks away
  from its Job Object. Normal dev tools don't do this.
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
- ✅ v0.3: Claude Code hook with prompt attribution.
- ✅ v0.4: rerun, notifications when a job ends (browser, desktop, webhook), no orphaned processes
  on Linux/macOS, projects and tags.
- ✅ v0.5: setup recipes for Codex, Cursor, Antigravity CLI / Gemini CLI and Aider.
- ✅ v0.6: dashboard: several logs open at once, tick jobs to copy or download them (Text, Markdown,
  JSON; full log), pause / stop / rerun many at once, search.
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
