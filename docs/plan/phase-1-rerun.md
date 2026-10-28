# Phase 1: Rerun

Reference patch: [`patches/phase-1-rerun.patch`](patches/phase-1-rerun.patch)

## Goal

Run a finished job's command again with one action, **in the same folder**, as a new job linked
to the old one.

## What the user sees

```text
$ ptrack rerun k3f9a2          # foreground, output shown, exits with the command's code
$ ptrack rerun k3f9a2 -d       # in the background, prints the new job id
$ ptrack rerun k3f9a2 -q       # quiet: one summary line
$ ptrack status                # shows "rerun of  k3f9a2"
```

- The dashboard shows a **Rerun** button on every finished job (passed, failed, cancelled, lost).
  It asks for confirmation, then starts the rerun in the background. Rerun jobs show
  "rerun of #k3f9a2".
- MCP: a new tool `job_rerun`, available **only** with `ptrack mcp --allow-control`.
- Rerunning a job that is still running or paused is refused: "job k3f9a2 is still running; stop
  it first".

## Design

- `Job` gets a new field `rerun_of: Option<String>`, with `#[serde(default)]` (AGENTS.md rule 6).
- `RunOpts` gets `cwd: Option<String>` and `rerun_of: Option<String>`. `ptrack run` accepts them
  as `--cwd DIR` and `--rerun-of ID`, which is also how the background supervisor receives them.
- The supervisor sets `job.cwd`, `job.rerun_of` and `cmd.current_dir(...)` from them.
- Code for starting a **background** job is moved out of `detach()` into a reusable
  `pub fn start_detached(opts: &RunOpts) -> Result<Detached, String>`. The dashboard and MCP need
  to start jobs without printing to the terminal. `detach()` becomes a thin wrapper that prints.
- `commands.rs` gets three helpers:
  - `rerun_opts(&Job) -> Result<RunOpts, String>`: builds the options; refuses active jobs.
  - `rerun(id, detach, quiet) -> CmdResult`: the CLI command.
  - `rerun_detached(&Job) -> Result<String, String>`: used by the dashboard and MCP; returns a
    message like `started job ab12cd (rerun of k3f9a2)`.
- The rerun copies `label`, `shell`, `command` and `cwd`. It does **not** copy agent, session or
  prompt: those describe whoever runs the rerun, and are detected again as usual.

## Steps

1. **`src/job.rs`**: in `struct Job`, after the `prompt` field, add:
   ```rust
   /// Set when this job was started by `ptrack rerun` of another job.
   #[serde(default)]
   pub rerun_of: Option<String>,
   ```
   In `Job::new`, add `rerun_of: None,` after `prompt: None,`.

2. **`src/supervise.rs`**:
   1. In `struct RunOpts`, after `prompt_id`, add:
      ```rust
      /// Folder to run in (default: the current folder). Used by `rerun`.
      pub cwd: Option<String>,
      /// Id of the job this one repeats (`ptrack rerun`).
      pub rerun_of: Option<String>,
      ```
   2. Replace the whole `fn detach(opts: RunOpts) -> i32` with three items, copied from the patch:
      - `pub enum Detached { Started(Box<Job>), NotYetReported(String) }`. The `Box` is needed
        because clippy's `large_enum_variant` lint fails without it.
      - `pub fn start_detached(opts: &RunOpts) -> Result<Detached, String>`, the old body, but
        returning values instead of printing. It also forwards `--cwd` and `--rerun-of`.
      - `fn detach(opts: &RunOpts) -> i32`, which prints exactly what it printed before.
        The exit codes are the same as before: 0 normally, 127 when the job failed to start.
   3. In `run()`, change `return detach(opts);` to `return detach(&opts);`.
   4. In `supervise()`, after `job.prompt = ...`, add:
      ```rust
      job.rerun_of = opts.rerun_of.clone();
      if let Some(dir) = &opts.cwd {
          job.cwd = dir.clone();
      }
      ```
   5. In `supervise()`, right after the `let mut cmd = if opts.shell { ... };` block, add:
      ```rust
      if let Some(dir) = &opts.cwd {
          cmd.current_dir(dir);
      }
      ```

3. **`src/main.rs`**:
   1. In `parse_run`, add two options next to `--prompt-id`:
      ```rust
      "--cwd" => opts.cwd = Some(p.value(&a)?),
      "--rerun-of" => opts.rerun_of = Some(p.value(&a)?),
      ```
   2. Add a `"rerun"` arm to `dispatch`, before `"pause" | "resume"`. It accepts `-d/--detach`,
      `-q/--quiet` and a required id, and calls `commands::rerun(&id, detach, quiet)`.
   3. Add to `HELP`, after the pause/resume line:
      `  ptrack rerun ID [-d] [-q]                     run a finished job's command again (same folder)`

4. **`src/commands.rs`**:
   1. Change `use crate::supervise::summary;` to `use crate::supervise::{self, RunOpts, summary};`.
   2. In `status()`, after the `prompt` row, add a `"rerun of"` row when `j.rerun_of` is set.
   3. Append `rerun_opts`, `rerun` and `rerun_detached`, copied from the patch.

5. **`src/web.rs`**: in the `POST /api/jobs/<id>/<action>` match, add:
   ```rust
   "rerun" => commands::rerun_detached(&j),
   ```

6. **`src/mcp.rs`**: inside `if allow_control { ... }` in `tool_list`, add the `job_rerun` tool
   definition. In `call_tool`, add a `"job_rerun"` arm that calls `control("job_rerun")?` first,
   then `commands::rerun_detached(&j)`.

7. **`src/ui.html`**:
   1. In `renderJob`, change `if (active) { ...Stop button... }` so that it has an `else` branch
      adding a **Rerun** button that calls `act(j, "rerun", e.target)`.
   2. In the `meta` element list, after the exit code, add
      `j.rerun_of ? el("span", {}, "rerun of #" + j.rerun_of) : null,`.
   3. In `act()`, after the stop confirmation, add a rerun confirmation:
      `if (action === "rerun" && !confirm("Run this again in " + job.cwd + "?\n\n" + title(job))) return;`

8. **`tests/cli.rs`**:
   1. In `mcp_server_lists_jobs_and_guards_control_tools`, the last assertion expects 7 tools
      with `--allow-control`. **Change 7 to 8.** This is an intentional behaviour change; mention
      it in the commit message.
   2. In `dashboard_requires_token_and_local_host`, after the `resume` → 409 assertion, add a
      `POST /api/jobs/last/rerun` that must return 200 with "rerun of" in the body.
   3. Add the two new tests from the patch:
      - `rerun_repeats_the_command_in_the_same_folder`: proves the folder is right by having the
        command create `here.txt`. It reruns from a different folder and checks that the file
        reappears in the original one.
      - `rerun_refuses_a_job_that_is_still_running`: expects exit code 2 and "still running".

9. **`README.md`**: add a `ptrack rerun` row to the Commands table, and a
   `ptrack rerun k3f9a2` line to the Quick start block.

## Manual checks (Windows PowerShell; use a throwaway data folder)

```powershell
$env:PTRACK_HOME = "$env:TEMP\ptrack-phase1"
cargo build
.\target\debug\ptrack.exe run -l "flaky" -s -- "echo hi & exit 3"   # exit code 3
.\target\debug\ptrack.exe rerun last                                 # exit code 3 again
.\target\debug\ptrack.exe status                                     # shows "rerun of  <first id>"
.\target\debug\ptrack.exe ui                                         # Rerun buttons visible; click one
```

## Pitfalls

- `Detached::Started` must hold `Box<Job>`, not `Job`, or clippy fails.
- Keep `detach()`'s printed text and exit codes unchanged: existing tests and users rely on them.
- Don't rerun with the original agent or session. Leave those to `detect_agent`.

## Commit message

```text
Add ptrack rerun (CLI, dashboard button, MCP job_rerun)

The MCP control tool count with --allow-control changes from 7 to 8.
```

## Done when

- [ ] `verify` passes; CI is green on all three systems
- [ ] The manual checks behave as described
- [ ] `ptrack help` lists `rerun`
