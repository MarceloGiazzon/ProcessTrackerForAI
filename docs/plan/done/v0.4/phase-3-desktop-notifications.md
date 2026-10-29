# Phase 3: Desktop notifications

Reference patch: [`patches/phase-3-desktop-notifications.patch`](patches/phase-3-desktop-notifications.patch)

## Goal

When a job ends, show a normal system notification, even if no dashboard is open. This is off by
default; `ptrack notify on` turns it on.

## What the user sees

```text
$ ptrack notify             # desktop notifications: off / only for jobs that ran at least: 30s / settings file: ...
$ ptrack notify on
$ ptrack notify min-seconds 60
$ ptrack notify test        # shows a test notification now, or explains why it can't
$ ptrack notify off
```

The notification title is `Job failed: npm test`. The body is `took 3m04s · exit 1`, then the last
output line.

## Design

- **Settings file** `<home>/config.json`, in a new module `src/config.rs`:
  ```json
  { "notify": { "desktop": false, "min_seconds": 30 } }
  ```
  - `config::load()` is used by the supervisor. **Any** problem means defaults, so a broken
    settings file can never break a job.
  - `config::load_checked()` is used by `ptrack notify`. It reports a broken file instead of
    silently overwriting it.
  - `#[serde(default)]` on both structs means missing keys get defaults, so phase 4 can add keys.
- **When:** at the very end of `supervise()`, right after the final `job.save()`, call
  `crate::notify::job_finished(&job)`. It does nothing if the job was cancelled, or ran less than
  `min_seconds`.
- **How**, on each system, with tools that already exist (no new dependency):
  - Windows: `powershell.exe -NoProfile -NonInteractive -Command <WINDOWS_TOAST>`, with the title
    and body in the environment variables `PTRACK_NOTIFY_TITLE` / `PTRACK_NOTIFY_BODY`, and
    `CREATE_NO_WINDOW` so no console flashes. The script is a constant. Job text is **never**
    pasted into it.
  - macOS: `osascript -e 'on run argv' -e 'display notification (item 2 of argv) with title (item 1 of argv)' -e 'end run' <title> <body>`.
  - Linux: `notify-send --app-name=ptrack -- <title> <body>`.
  - The title always starts with "Job ", so it can never be read as a command-line option.
- **Don't wait** for the notifier: spawn it and return, and it finishes on its own.
- **Critical:** call `crate::platform::stop_sharing_std_handles()` before spawning.

### Why `stop_sharing_std_handles()` is critical (a bug found while writing this plan)

On Windows, a new process inherits every inheritable handle of its parent, including ptrack's own
stdout/stderr. When an agent runs a (hook-wrapped) command and captures its output through a pipe,
it waits until **every** holder of that pipe has closed it. Without this call, the agent also
waits for PowerShell to finish showing the toast, measured at +0.75 s per command. With a webhook
(phase 4) it could be up to 10 s.

The fix already existed inside `platform::windows::spawn_detached`. This phase moves it into its
own function, `pub fn stop_sharing_std_handles()`, in `src/platform/windows.rs`. It also adds an
empty function with the same name to `src/platform/unix.rs`, where Rust already avoids the problem.
Phase 4 adds a test that fails if this call is ever removed.

## Steps

1. **New file `src/config.rs`**: copy it from the patch (`Config`, `NotifyConfig` with `desktop`
   and `min_seconds`, `path()`, `load()`, `load_checked()`, `save()`).
2. **New file `src/notify.rs`**: copy it from the patch: `WINDOWS_TOAST`, `job_finished`,
   `message`, `desktop_command`, `command` (the CLI), `show`, `send_test`, and a unit test.
3. **`src/main.rs`**:
   - Add `mod config;` and `mod notify;` (keep the `mod` lines in alphabetical order).
   - Add `"notify" => notify::command(&args.into_iter().collect::<Vec<_>>()),` to `dispatch`.
   - Add two `HELP` lines (see the patch).
4. **`src/supervise.rs`**: after the final `if let Err(e) = job.save() { ... }`, add
   `crate::notify::job_finished(&job);`.
5. **`src/platform/windows.rs`**: in `spawn_detached`, replace the `for std_handle in [...]` loop
   (and its comment) with a call to `stop_sharing_std_handles();`. Move the loop into the new
   `pub fn stop_sharing_std_handles()` at the end of the file, with its comment.
6. **`src/platform/unix.rs`**: add `pub fn stop_sharing_std_handles() {}` with a comment
   explaining why nothing is needed.
7. **`src/platform/mod.rs`**: add `stop_sharing_std_handles` to the list in the module comment.
8. **`tests/cli.rs`**: add `notify_settings_are_saved_and_validated`. It checks saving, bad
   values (exit 2), and that a broken `config.json` is reported, left untouched, and doesn't break
   `ptrack run`.
9. **`README.md`**: add the "Notifications when a job ends" section before "MCP server", and a
   Commands table row (see the patch).

## Manual checks (Windows; these show real pop-ups)

```powershell
$env:PTRACK_HOME = "$env:TEMP\ptrack-phase3"
cargo build
.\target\debug\ptrack.exe notify test            # a "Job passed: ptrack test" pop-up appears
.\target\debug\ptrack.exe notify on
.\target\debug\ptrack.exe notify min-seconds 0
.\target\debug\ptrack.exe run -l 'deploy <x> & "y"' -s -- "exit 1"   # pop-up shows the title literally
.\target\debug\ptrack.exe notify off
```

Then check the timing fix by hand, in Git Bash:

```sh
export PTRACK_HOME=/tmp/ptrack-phase3; ./target/debug/ptrack notify on; ./target/debug/ptrack notify min-seconds 0
time (out=$(./target/debug/ptrack run -q -s -- "echo x" 2>&1))   # must be well under 0.6 s
```

On Linux/macOS, ask the owner to run `ptrack notify test` on their notebook. CI machines have no
desktop.

## Pitfalls

- **Never** add `-ExecutionPolicy Bypass` (or `-ep bypass`). It isn't needed for `-Command`, and
  security tools flag it.
- Use `powershell.exe` (Windows PowerShell 5.1), **not** `pwsh`. The toast API (`ContentType =
  WindowsRuntime`) only exists in 5.1, which every Windows 10/11 has.
- Keep the app id string in `WINDOWS_TOAST` exactly as it is. It is the id Windows registers for
  PowerShell, which is why the toast is allowed to show.
- `job_finished` must never panic or print. Errors are ignored on purpose: notifications are best
  effort.

## Commit message

```text
Desktop notifications when a job ends (ptrack notify)

Also: notifier processes no longer inherit ptrack's stdout/stderr on
Windows (stop_sharing_std_handles), so callers capturing output don't wait
for them.
```

## Done when

- [ ] `verify` passes; CI is green
- [ ] `ptrack notify test` shows a pop-up on Windows
- [ ] The timing check is well under 0.6 s with notifications on
