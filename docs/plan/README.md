# Implementation plan: v0.4 features

This plan is for the AI coding tool that will implement the next features of `ptrack`. Follow it
literally, one phase at a time. When something here seems unclear or wrong, stop and ask the owner
instead of guessing.

**Before anything else, read [`AGENTS.md`](../../AGENTS.md) in the repository root.** Its rules
apply to every step below.

## What gets built

| Phase | Feature | What the user gets | Risk | Plan |
|---|---|---|---|---|
| 1 | Rerun | `ptrack rerun ID`, a **Rerun** button in the dashboard, an MCP `job_rerun` tool | Low | [phase-1-rerun.md](phase-1-rerun.md) |
| 2 | Browser notifications | A "Notify when jobs end" toggle in the dashboard | Low | [phase-2-browser-notifications.md](phase-2-browser-notifications.md) |
| 3 | Desktop notifications | `ptrack notify on`: a system notification when a job ends | Medium | [phase-3-desktop-notifications.md](phase-3-desktop-notifications.md) |
| 4 | Webhook | `ptrack notify webhook URL`: POST each finished job (Slack, Discord, ntfy, scripts) | Medium | [phase-4-webhook.md](phase-4-webhook.md) |
| 5 | Unix guard | No leftover processes on Linux/macOS if the supervisor is killed | **High** | [phase-5-unix-guard.md](phase-5-unix-guard.md) |

Do the phases **in this order**. Each one builds on the files changed by the previous one, and the
easier phases come first. Phases 1–4 can be fully checked on Windows. Phase 5 is Unix-only and is
proven only by GitHub CI after you push.

## Reference patches: use them

Every phase has a **reference patch** in [`patches/`](patches/). It is a complete, working
implementation of that phase, already checked with the full `verify` script (formatting, lint for
Windows, Linux and macOS, all tests on Windows). It was made against commit `3b3d21c` plus this
plan.

The most reliable way to do a phase:

1. Make sure the working tree is clean: `git status` shows nothing to commit.
2. Apply the phase's patch to the working tree, without committing:
   ```sh
   git apply --3way docs/plan/patches/phase-1-rerun.patch
   ```
   - If it applies cleanly, go to step 3.
   - If it reports conflicts, or "patch does not apply", the code changed since the plan was
     written. Run `git checkout -- .` to undo, then implement the phase **by hand**, following the
     numbered steps in the phase file and using the patch as the model for each change.
3. **Read the whole diff** (`git diff`) next to the phase file, so you understand every change.
   You'll need that understanding if CI fails.
4. Run the checks (see below), then do the phase's **manual checks**.
5. Commit with the commit message given in the phase file, push, and wait for CI to pass.

Don't apply several patches before committing. One phase means one commit.

## Checks to run (every phase)

```sh
cargo fmt                 # always first: fixes formatting
sh scripts/verify.sh      # Git Bash / Linux / macOS   (or: ./scripts/verify.ps1 in PowerShell)
```

`verify` must end with `ALL CHECKS PASSED`. Then:

```sh
git add -A
git commit -m "<message from the phase file>"
git push origin main
gh run list --limit 1            # note the run id
gh run watch <run-id> --exit-status
```

A phase is **done only when CI is green on all three systems** (Windows, Linux, macOS).

## When to stop and ask the owner

Stop, and report what you tried and the exact error, if any of these happen:

- The same check fails three times in a row.
- A fix seems to need a new crate (dependency), or disabling or loosening a test.
- A fix seems to need weakening a security rule from `AGENTS.md`.
- CI fails on Linux or macOS and the phase file's troubleshooting section doesn't explain it.
- The owner's real data might be affected. Always use a throwaway `PTRACK_HOME` for manual
  checks.

## Design decisions already made

Don't "improve" these. Each was chosen on purpose, for the reason given:

| Decision | Why |
|---|---|
| No new dependencies; notifications use PowerShell, osascript, notify-send and curl | The product promise is "one file, nothing to install" |
| Desktop notifications are **off** by default | Pop-ups for every agent command would be noise |
| Only jobs that ran ≥ 30 s notify, and cancelled jobs never do | Whoever stopped a job already knows; short jobs aren't worth a pop-up |
| Job text reaches notifiers through environment variables and arguments only | Text in script source could be run as code |
| Webhook URLs must start with `http://` or `https://` and are passed with `--url` | Anything else could be read by curl as an option or a local file |
| Notifiers are started without waiting, after `stop_sharing_std_handles()` | Otherwise an agent capturing ptrack's output waits for the notifier too; a test proves it |
| The Unix guard is a second ptrack process, not a library | It must survive the supervisor's death to clean up after it |
| Rerun refuses jobs that are still running | Two copies of a server or build in one folder usually collide |

## After all phases

1. `Cargo.toml` already says `version = "0.4.0"` (phase 5 sets it).
2. Install the new build on the owner's machine (see `AGENTS.md`, "How to check your work").
3. Tell the owner it is ready. **The owner** creates the release by pushing the tag `v0.4.0`.
   Never push tags yourself.

## What was and wasn't verified before you started

| Item | Status |
|---|---|
| All 5 patches: format, lint on Windows, Linux and macOS, all tests on Windows | ✅ Passed |
| Windows desktop toast (PowerShell), including text with `<`, `&` and quotes | ✅ PowerShell reported success on the owner's PC (owner to confirm the pop-up appeared) |
| Webhook delivery with curl, the `notify test` command, the hanging-webhook regression test | ✅ Passed on Windows |
| The hanging-webhook test really catches the bug (fix removed → the test fails after 10 s) | ✅ Confirmed |
| Phase 5 at runtime on Linux/macOS (the guard tests) | ⚠️ **Not run yet**: only compiled. CI runs them |
| macOS (`osascript`) and Linux (`notify-send`) desktop notifications on a real desktop | ⚠️ Not run; CI has no desktop. Ask the owner to try `ptrack notify test` |
| Browser notifications on a real browser | ⚠️ Manual check in phase 2 |
