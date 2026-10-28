# Phase 5: Unix guard (no orphaned processes on Linux/macOS)

Reference patch: [`patches/phase-5-unix-guard.patch`](patches/phase-5-unix-guard.patch)

**This is the riskiest phase.** It changes how every job starts on Linux and macOS, and its tests
can only run in GitHub CI (on Windows this code is only compiled). Read this whole file before
starting.

## The problem

On Linux/macOS, the supervisor starts the command in its own process group. If the supervisor is
killed with `kill -9` (or crashes), nothing stops that group: the job's processes keep running,
and ptrack shows the job as `lost`. Windows doesn't have this problem, because the Job Object has
`KILL_ON_JOB_CLOSE`.

## The solution: a guard process

```text
supervisor (ptrack run)
   │  stdin pipe = "lifeline"  (only the supervisor holds the write end)
   ▼
ptrack __guard -- <command...>      ← leader of the job's process group
   └── <command> and its children   ← same process group
```

- The supervisor no longer starts the command directly. It starts
  `<ptrack exe> __guard -- <program> <args...>` as the leader of a new process group. The
  guard's **stdin** is the read end of a pipe whose write end only the supervisor holds (the
  "lifeline").
- The guard starts the real command (stdin `/dev/null`, inheriting stdout/stderr, which are the
  supervisor's output pipes), then waits for it.
- **Normal case:** the command exits, and the guard exits with the same code (128+N if killed by
  signal N). The supervisor sees exactly what it saw before.
- **Supervisor dies:** the kernel closes the lifeline, and the guard's read on stdin returns
  end-of-file. The guard then sends SIGTERM + SIGCONT to the whole group. After 5 s it sends
  SIGKILL, which also ends the guard.
- **Stop and pause still work unchanged.** They signal the whole group. The guard ignores
  SIGTERM, SIGINT and SIGHUP, set *after* starting the command so the command doesn't inherit
  "ignored". So a stop reaches the command, and the guard reports how it ended.
- **Paused job + supervisor killed:** when the supervisor dies, the group becomes "orphaned"
  (POSIX). The kernel then sends SIGHUP + SIGCONT to its stopped members. The guard wakes up,
  ignores SIGHUP, sees end-of-file and cleans up. A test covers this.
- **Windows is unchanged.** The new `prepare` returns `()` as its lifeline there.

`std::io::pipe()` needs Rust 1.87, so `rust-version` in `Cargo.toml` goes from 1.85 to 1.87.

## Steps

1. **`Cargo.toml`**: `rust-version = "1.87"`, and `version = "0.4.0"` (this is the last phase).
2. **New file `src/guard.rs`**: copy it from the patch. Every line matters; the comments explain
   why. Key points:
   - Spawn the command **before** ignoring the signals.
   - A thread reads stdin until end-of-file, then sets `supervisor_gone`.
   - The main loop: `try_wait` every 100 ms, then SIGTERM + SIGCONT once, then SIGKILL after
     `GRACE`.
   - When the command exits after the supervisor is gone: SIGKILL the group (leftovers).
3. **`src/main.rs`**:
   ```rust
   #[cfg(unix)]
   mod guard;
   ```
   and in `dispatch`, next to `"notify"`:
   ```rust
   // Internal, started by the supervisor on Unix: see guard.rs.
   #[cfg(unix)]
   "__guard" => {
       let command: Vec<String> = args.into_iter().skip_while(|a| a != "--").skip(1).collect();
       Ok(guard::run(&command))
   }
   ```
   Don't add it to `HELP`: it is internal.
4. **`src/platform/unix.rs`**: replace `pub fn prepare(cmd: &mut Command)` with
   `pub type Lifeline = io::PipeWriter;` and
   `pub fn prepare(cmd: Command) -> io::Result<(Command, Lifeline)>`, which builds the guard
   command. It **must copy** the program, the args, `current_dir` (phase 1's `--cwd`) and any
   env changes from the original command (see the patch).
5. **`src/platform/windows.rs`**: change `prepare` to the same signature:
   `pub type Lifeline = ();` and
   `pub fn prepare(mut cmd: Command) -> io::Result<(Command, Lifeline)>`, which sets the same
   creation flags as before **plus `stdin(Stdio::null())`**. That line moved here from the
   supervisor.
6. **`src/supervise.rs`**: replace
   ```rust
   cmd.stdin(Stdio::null())
       .stdout(Stdio::piped())
       .stderr(Stdio::piped());
   platform::prepare(&mut cmd);
   ```
   with
   ```rust
   // `_lifeline` must stay alive until this function returns: on Unix,
   // dropping it tells the guard process that the supervisor is gone.
   // (Naming it `_` instead would drop it immediately.)
   let (mut cmd, _lifeline) = match platform::prepare(cmd) {
       Ok(prepared) => prepared,
       Err(e) => {
           eprintln!("ptrack: cannot prepare the command: {e}");
           return 1;
       }
   };
   cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
   ```
   ⚠️ Write `_lifeline`, **never** `_`. A bare `_` drops the pipe at once: every guard would then
   think its supervisor died and kill every job within a second.
7. **`tests/cli.rs`**: add the Unix-only helpers `group_alive`, `pid_alive`, `eventually` and
   `kill_supervisor` (they use `libc::kill` directly, which works with any `/bin/sh`), and three
   tests:
   - `killing_the_supervisor_stops_the_job_on_unix`
   - `a_paused_job_is_cleaned_up_too_when_its_supervisor_dies`
   - `background_helpers_survive_a_normal_exit_on_unix`: a normal exit must **not** kill
     helpers, the same behaviour as Windows (for example a Gradle daemon).
8. **`README.md`**:
   - In the "Process control per system" table, change the "If the supervisor itself is killed"
     row.
   - In "Honest limits", replace the "On Linux/macOS, a killed supervisor can leave the job
     running" bullet with the one about programs that detach themselves.
   - Update the Roadmap.
   (See the patch for all three.)

## Checking this phase

1. `cargo fmt` and `verify`. On Windows this lints the Unix code for Linux and macOS, but **cannot
   run** it.
2. Commit, push, and watch CI: `gh run watch <id> --exit-status`.
3. CI must be green on **ubuntu-latest and macos-latest**. Those are the only machines that run
   the 3 new tests and the existing tests through the guard.

## If CI fails on Linux or macOS

Read the failing test's output with `gh run view <id> --log-failed`. Then:

| Symptom | Likely cause | Fix |
|---|---|---|
| Almost every Unix test fails; long jobs end within a second as `failed` with exit code 143 (SIGTERM) | The lifeline is dropped early (`_` instead of `_lifeline`, or it was moved into a short scope) | Keep `_lifeline` bound until the end of `supervise()` |
| `missing_program_fails_cleanly` fails | The guard must return **127** when the command can't start | Check the `Err(e) => { eprintln!(...); return 127; }` arm in `guard::run` |
| `rerun_repeats_the_command_in_the_same_folder` fails on Unix | `prepare` didn't copy `current_dir` into the guard command | Copy `cmd.get_current_dir()` |
| `stop_cancels_a_running_job` fails (job not cancelled) | Signals were ignored **before** the spawn, so the command ignores SIGTERM too | Ignore signals only after `spawn()` succeeds |
| The paused-job test fails only on macOS | The orphaned-group wake-up behaves differently | Stop and report to the owner, with the log. Don't delete or `#[ignore]` the test |
| `error[E0658]` / `io::pipe` not found | Old Rust on CI | CI installs the latest stable Rust; check `rust-version` and the workflow's `rustup update stable` step |

If a fix isn't obvious after two attempts, **stop and report**. Don't weaken or skip tests.

## Commit message

```text
Unix: guard process stops the job if its supervisor dies

Jobs on Linux/macOS now run under `ptrack __guard`, which holds a pipe to
the supervisor and stops the job's process group if the supervisor dies.
Requires Rust 1.87 (std::io::pipe). Version 0.4.0.
```

## Done when

- [ ] `verify` passes on Windows
- [ ] CI is green on Windows, Linux and macOS, including the 3 new Unix tests
- [ ] Optional, if the owner's Ubuntu notebook is available: run
      `ptrack run -d -- sleep 300`, then `kill -9 <supervisor_pid from ptrack status --json>`.
      Within a few seconds, `pgrep sleep` no longer shows it, and after about 10 s
      `ptrack status` says `lost`.
