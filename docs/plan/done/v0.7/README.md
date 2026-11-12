# Plan: v0.7, no job is ever blind (done: released as v0.7.0 on 2026-11-12)

Asked for by the NPDev repository on 2026-11-12 (against ptrack 0.6.0). Goal: for any job,
`ptrack tail -f` and the LAST LINE column of `ptrack list` answer within 5 seconds: is it alive?
slow or stuck? what is it doing? where did it end up (exit, duration)?

Evidence: 96 of 771 job logs on the owner's machine were 0 bytes. Causes: output redirected by the
command (`> x.log`), filters that printed nothing, commands that are silent by nature (`cp`). Only
ptrack sees every job, so only ptrack can fix the last three.

Read [`AGENTS.md`](../../../../AGENTS.md) first. One commit per phase, after `verify` passes, pushed,
and CI green.

| Phase | Request | What changes | Commit message |
|---|---|---|---|
| 1 | P1 | Log header (`# ptrack <id> <label>`, `# cmd:`, `# cwd:`, `# pid:`, `# started:`) and footer (`# ended: ... exit= duration= output= state=`). `last_line` is `(no output)` for a silent job. Failure excerpts skip ptrack's own lines. | `Logs: header and footer, so no log is ever empty` |
| 2 | P2, P3 | CPU time of the job's whole process tree. `# [ptrack] hh:mm:ss running, no output for 2m00s, tree cpu=41s` heartbeat lines in the log while the command is silent (default every 30 s, `PTRACK_HEARTBEAT_SECONDS` or `ptrack config heartbeat-seconds N`, 0 = off). The child gets `PTRACK_JOB_ID`, `PTRACK_LOG`, `PTRACK_HEARTBEAT_SECONDS`. | `Logs: heartbeat lines while a job is silent; tell the job its id and log` |
| 3 | P4 | Activity of a running job: `running` (output in the last 60 s), `quiet` (no output, but the tree still uses at least 1% of one core), `stalled?` (no output and no CPU for 300 s; advice only, never acted on). In `list`, `status`, `--json`, the dashboard and MCP. `ptrack config quiet-after N` / `stalled-after N`. | `Show whether a silent job is slow (quiet) or probably hung (stalled?)` |
| 4 | P5 | When `list`/`status`/... see a job as `lost`, they also record `"state": "lost"` in its job file. This is a written exception to rule 1 (approved by the owner on 2026-11-12). | `Record lost jobs as lost in their job file` |

Not asked for: killing stalled jobs, notifications about them, any change to how output is captured.

## Decisions

- ptrack's own log lines all start with `# `. Readers that want only the command's output use
  `output::last_output_lines`, which drops them.
- `state` in the job file and JSON stays `running` for quiet or stalled jobs, so existing readers
  keep working. The activity is a separate field (`activity`), computed when the job is read.
- Tree CPU: Windows reads the Job Object's accounting, which includes processes that have already
  exited. Linux and macOS add up the process group's CPU time, including children each member has
  already reaped. Work done outside the tree (a Gradle daemon that is not a descendant) is not
  seen; that is why it says `stalled?` with a question mark.
