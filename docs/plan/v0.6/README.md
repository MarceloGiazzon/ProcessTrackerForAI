# Implementation plan: v0.6 dashboard improvements

This plan is for the AI coding tool that will implement the next dashboard features of `ptrack`.
Follow it one phase at a time. When something here seems unclear or wrong, stop and ask the owner
instead of guessing.

**Before anything else, read [`AGENTS.md`](../../../AGENTS.md) in the repository root.** Its rules
apply to every step below. The ones that matter most here:

- **No new dependencies.** Everything below is plain browser JavaScript plus the standard library.
- **Never weaken security** (token on every `/api/` call, localhost `Host` check, loopback only).
  No phase adds a way around them. Phase 2 adds one new *query option* to an existing token-checked
  endpoint, and nothing else on the server.
- **Single writer.** Batch actions (phase 4) call the existing per-job endpoints. The dashboard
  never writes job files.
- **Every user-visible feature needs** a line in `HELP` (the `ptrack ui` entry in `src/main.rs`), a
  README section (the "Dashboard" chapter) and a test in `tests/cli.rs` or a unit test.
- Work on `main`, no branches. One commit per phase, after `verify` passes. Push after each commit
  and wait for CI.

## Starting point

Commit `848e8c3` ("Dashboard: open several jobs, tick jobs, copy or download their output"). The
dashboard already has:

- `openJobs` (Set), `logs` (Map: id → `{ text, final, top, stick }`) and `selected` (Set) in
  `src/ui.html`. Several logs can be open at once.
- `shown`: the jobs on screen right now, in order. **Every bulk feature must act on `shown`**, never
  on a separate list, so filters and search apply to all of them automatically.
- A bar (`#bar`) with *Select all*, the "N of M selected" text, **Copy selected** and **Download
  selected**. Both use `selectedText()`, which fetches `/api/jobs/ID/log?lines=5000`.
- The server (`src/web.rs`) caps `lines` at 5000 and builds the whole response body in memory
  (`respond(&mut stream, code, content_type, &str)`).

## What gets built, in the order to build it

| Phase | Feature | What the user gets | Touches | Risk |
|---|---|---|---|---|
| 1 | Expand all / Collapse all | Two buttons to open or close every log on screen | `ui.html` | Low |
| 2 | Full log export | Copy / Download can take the whole log, not just the last 5000 lines | `util.rs`, `web.rs`, `ui.html` | Medium |
| 3 | Export formats | Choose Text, Markdown or JSON for Copy and Download | `ui.html` | Low |
| 4 | Batch actions | Pause / Resume / Stop / Rerun for all ticked jobs at once | `ui.html` | Medium |
| 5 | Search | A search box that filters jobs, plus highlighted matches in open logs | `ui.html` | Medium |

### Why this order

1. **Expand/Collapse all first.** It is the smallest change and touches only state that already
   exists (`openJobs`, `logs`). It is a safe warm-up and gives the owner something to try at once.
   It also fixes the rule "bulk buttons act on `shown`" before the bigger phases rely on it.
2. **Full log (2) before formats (3).** Phase 2 changes *where the text comes from* (a new server
   option and a size limit). Phase 3 changes *how the text is wrapped* (text, Markdown, JSON). If
   formats came first, the JSON/Markdown writers would have to be revisited for the "truncated"
   flag that phase 2 introduces. Doing 2 first means phase 3 is written once, against the final data
   shape.
3. **Batch actions (4) after the export cluster.** It is the first phase that can do something
   hard to undo (Stop, Rerun many jobs). By then the selection model has been exercised by three
   phases, and the confirmation pattern can be copied from the existing `act()` function. It needs
   no server change, so it is independent of 2 and 3 and could be swapped with them if the owner
   prefers actions over exports.
4. **Search last (5).** It is the largest UI change: it splits `refresh()` into "fetch" and
   "render", adds a keyboard shortcut and highlights text inside logs. Doing it last means that it
   re-renders a page whose features are all finished, and the earlier phases cannot be broken by the
   split. Because every earlier feature works on `shown`, search needs only one extra line to set
   `shown` to the filtered list.

Each phase is independently releasable: stop after any phase and the dashboard is consistent.

## Checks to run (every phase)

```sh
cargo fmt                 # always first: fixes formatting
sh scripts/verify.sh      # Git Bash / Linux / macOS   (or: ./scripts/verify.ps1 in PowerShell)
```

`verify` must end with `ALL CHECKS PASSED`. Then also check the JavaScript syntax, because `verify`
does not look inside `ui.html`:

```sh
sed -n '/^<script>/,/^<\/script>/p' src/ui.html | sed '1d;$d' > "$TEMP/ui-check.js" && node --check "$TEMP/ui-check.js"
```

(Skip this if `node` is not installed, and say so in your report.) Then do the phase's **manual
checks** in a real browser against a throwaway data folder:

```sh
PTRACK_HOME=/tmp/ptrack-try ./target/debug/ptrack ui          # Unix shells
$env:PTRACK_HOME="$env:TEMP\ptrack-try"; .\target\debug\ptrack.exe ui   # PowerShell
```

Create a few jobs first in that same folder (`ptrack run -- echo hi`, one that fails, one that
sleeps, one that prints 20000 lines). Never point manual checks at the owner's real data.

Commit, push and wait for CI:

```sh
git add -A
git commit            # message from the phase, ending with your tool's attribution line
git push origin main
gh run list --limit 1
gh run watch <run-id> --exit-status
```

A phase is **done only when CI is green** on all three systems.

## When to stop and ask the owner

- The same check fails three times in a row.
- A fix seems to need a new crate, or loosening a test, or weakening a security rule.
- CI fails on Linux or macOS and you cannot see why from the log.
- You are about to change behaviour this plan says to keep (for example the default of a setting).

---

## Phase 1: Expand all / Collapse all

**Goal.** Two buttons in the bar open or close the log of every job on screen.

### Design decisions

- Buttons live in the bar (`#bar`), after the selection text: **Expand all** and **Collapse all**.
  They are independent of the checkboxes (opening is not selecting).
- *Expand all* opens every job in `shown`. *Collapse all* closes every open job (`openJobs.clear()`
  and `logs.clear()`), including ones currently filtered out, so nothing stays open invisibly.
- **Load limit.** Each open job that is still running re-fetches its log every 1.5 s. To avoid
  hammering the server, *Expand all* opens at most **20** jobs. If `shown.length > 20`, open the
  first 20 and show a toast: `Opened the first 20 of N jobs.` Finished jobs fetch once (`final`).
- Disable *Expand all* when every job in `shown` is already open; disable *Collapse all* when
  `openJobs` is empty. Update both in `updateBar()`.

### Steps

1. `src/ui.html`, HTML: add `<button id="expand">Expand all</button>` and
   `<button id="collapse">Collapse all</button>` to `#bar`, before the `Copy selected` button (keep
   the `<span class="spacer">` so the buttons stay on the right).
2. JS: add `const MAX_EXPAND = 20;` near the other state.
3. JS: handlers.
   ```js
   $("expand").addEventListener("click", () => {
     const closed = shown.filter((j) => !openJobs.has(j.id));
     for (const j of closed.slice(0, MAX_EXPAND)) openJobs.add(j.id);
     if (closed.length > MAX_EXPAND) toast("Opened the first " + MAX_EXPAND + " of " + closed.length + " jobs.");
     refresh();
   });
   $("collapse").addEventListener("click", () => { openJobs.clear(); logs.clear(); refresh(); });
   ```
4. JS: in `updateBar()`, set `$("expand").disabled = shown.every((j) => openJobs.has(j.id))` and
   `$("collapse").disabled = openJobs.size === 0`.
5. README "Dashboard": one sentence about the two buttons and the limit of 20.
6. `HELP`: extend the `ui` entry only if the line still reads well (it already mentions ticking jobs).
7. Test, `tests/cli.rs`, in `dashboard_requires_token_and_local_host`: assert the page contains
   `id="expand"` and `id="collapse"`.

### Manual checks

- With 3 jobs: *Expand all* opens three logs; *Collapse all* closes them; both buttons disable
  correctly.
- Open `ptrack ui --job <id>`: that job is open on load, *Collapse all* closes it.
- With 25+ jobs (create them with `ptrack run -q -- echo x` in a shell loop): the toast appears and only 20
  open.
- A log scrolled up by hand stays where it was after the periodic refresh (no regression).

**Commit message:** `Dashboard: Expand all / Collapse all for job logs`

---

## Phase 2: Full log export

**Goal.** *Copy selected* and *Download selected* can include the entire log of each job, not only
the last 5000 lines.

### Design decisions

- **Server.** `GET /api/jobs/ID/log?lines=all` returns the log's tail of at most **32 MiB**
  (`EXPORT_LOG_LIMIT`). If the file is bigger, the response starts at the first complete line inside
  that window and carries the header `X-Ptrack-Truncated: 1`. Anything else about the endpoint stays
  as it is (`lines=N` still capped at 5000; the in-page log panels keep using `lines=200`).
  Why a cap and not streaming: the whole page works on strings in memory, and a log can be several
  gigabytes. 32 MiB per job keeps both the server and the browser safe.
- No new route, no new file access: the endpoint already reads `j.log_path` after
  `job::resolve`. Keep the token and `Host` checks exactly as they are.
- **Client.** One new setting in the bar, a `<select id="scope">`: `Full log` (default) /
  `Last 5000 lines`. It is remembered in `localStorage` (key `ptrack.scope`, wrapped in `try/catch`
  like the others). It applies to Copy **and** Download. The owner asked for the full log, so Full is
  the default; if they prefer to keep the old default, flip it here.
- When a job's log was truncated, the export says so (see step 6), never silently.

### Steps

1. `src/util.rs`: add
   ```rust
   /// The end of a file, at most `max_bytes` long, starting at a line boundary.
   /// The bool tells whether the start of the file was cut off.
   pub fn tail_bytes(path: &Path, max_bytes: u64) -> std::io::Result<(String, bool)>
   ```
   - Open, read `len`. If `len <= max_bytes`, read everything and return `(text, false)`.
   - Otherwise seek to `len - max_bytes`, read to the end, then **drop everything up to and
     including the first `\n`** so the text starts on a whole line. Return `(text, true)`.
   - Convert with `String::from_utf8_lossy`: the cut may fall inside a multi-byte character, which
     lossy conversion turns into `?` at the very start; dropping the first partial line already
     removes it.
   - No `unwrap()` on file operations.
2. `src/web.rs`: add `const EXPORT_LOG_LIMIT: u64 = 32 * 1024 * 1024;`.
3. `src/web.rs`: give `respond` a sibling that takes extra headers, and make `respond` call it:
   ```rust
   fn respond_with(stream, code, content_type, extra: &[(&str, &str)], body: &str) -> io::Result<()>
   ```
   (`extra` headers are written after `Content-Length`. Keep all the existing security headers.)
4. `src/web.rs`, route `("GET", ["jobs", id, "log"])`: if `query_param(&req.query, "lines") ==
   Some("all")`, call `tail_bytes(Path::new(&j.log_path), EXPORT_LOG_LIMIT)` and respond with
   `respond_with(..., &[("X-Ptrack-Truncated", "1")] or &[], &text)`. A read error answers 404 with
   the error text, like the other branch. Otherwise, the current code path.
5. `src/ui.html`: add the select to `#bar` (labels as above) and load/save it like `showAll`.
6. `src/ui.html`, `selectedText()`: pick the query from the select
   (`lines=all` or `lines=5000`). Read `r.headers.get("X-Ptrack-Truncated") === "1"` and, when
   true, put `(log truncated: only the last 32 MB are included)` on the line after the header block
   and before the output. Keep the "(log unavailable)" fallback.
7. README: describe the scope menu and the 32 MB limit.
8. Tests:
   - Unit tests in `src/util.rs` for `tail_bytes`: small file (not truncated); file larger than the
     limit (truncated, result starts at a line start, last line intact); a limit that cuts exactly
     on a newline; a multi-byte character (`é`) split by the cut; empty file.
   - Integration test in `tests/cli.rs`, extending the dashboard test or a new one using the same
     helper (`http(...)`): create a finished job, then **append more than 5000 lines directly to its
     log file** (`job.log_path` from `ptrack status ID --json`, or build the path from
     `<home>/logs/<id>.log`; the supervisor has finished, so this is safe in a test). Assert that
     `?lines=5000` returns 5000 lines, that `?lines=all` returns all of them, and that
     `X-Ptrack-Truncated` is absent. To test truncation without writing 32 MB, make the limit a
     parameter of a small pure function and test that function in the unit tests rather than the
     server.
   - The page contains `id="scope"`.

### Manual checks

- A job with 20000 lines: Download with *Full log* has 20000 lines; with *Last 5000 lines*, 5000.
- Copy works with *Full log* (paste into an editor and check the last line).
- A log bigger than 32 MB (for example `ptrack run -- <command printing 400 MB>`): the file says it
  was truncated, the browser stays responsive.
- `curl` the endpoint without the token: still 401; with a wrong `Host`: still 403.

**Commit message:** `Dashboard: export the full log of selected jobs (up to 32 MB each)`

---

## Phase 3: Export formats

**Goal.** Copy and Download can produce **Text** (today's format), **Markdown** or **JSON**.

### Design decisions

- A second select in the bar, `<select id="format">`: `Text` (default) / `Markdown` / `JSON`,
  remembered as `ptrack.format`. It applies to Copy and Download.
- **Refactor first.** Split `selectedText()` into two functions so the formats are independent of
  the fetching:
  - `collectSelected()` returns `[{ job, output, truncated, available }]` (fetching as in phase 2).
  - `formatExport(items, format)` returns `{ text, mime, ext }`.
- Always plain text on the clipboard (`text/plain`), whatever the format.
- ANSI codes are already stripped by ptrack when it writes the log, so nothing to clean here.

### Formats

**Text** (unchanged from today; blocks separated by a blank line):
```
=== job k3f9a2: cargo test ===
state: passed · took 42s
folder: D:\proj
started: 2026-10-30T12:00:00.000Z
log: C:\...\logs\k3f9a2.log

<output>
```

**Markdown** (`.md`, `text/markdown`):
````
# ptrack export

Exported 2026-10-30T12:05:00.000Z · 2 jobs

## k3f9a2 · cargo test

- State: passed (exit 0) · took 42s
- Project: NPDev · Tag: CI Watch      (only the parts that exist)
- Folder: `D:\proj`
- Started: 2026-10-30T12:00:00.000Z
- Log: `C:\...\logs\k3f9a2.log`

```
<output>
```
````
- The title goes in the heading as plain text (it follows `## id · `, so a `#` inside it is harmless).
- **Fence length:** find the longest run of backticks in the output and use a fence one longer, with
  a minimum of 3. A log that contains ``` must not break the file. Write a small helper
  `fenceFor(text)`.
- A truncated or unavailable log adds an italic line under the list: `_Log truncated: only the last 32 MB._`

**JSON** (`.json`, `application/json`), pretty-printed with 2 spaces:
```json
{
  "exported_at": "2026-10-30T12:05:00.000Z",
  "jobs": [
    {
      "id": "k3f9a2", "title": "cargo test", "command": ["cargo", "test"],
      "state": "passed", "exit_code": 0, "duration_ms": 42000,
      "project": "NPDev", "tag": "CI Watch", "agent": "claude_code", "cwd": "D:\\proj",
      "started": "2026-10-30T12:00:00.000Z", "log_path": "C:\\...\\k3f9a2.log",
      "log_truncated": false, "output": "..."
    }
  ]
}
```
- Use fields already present in the `/api/jobs` JSON; `null` for the ones a job doesn't have.
- `JSON.stringify` does the escaping. Never build JSON by string concatenation.

### Steps

1. `src/ui.html`: add the select; store/restore it.
2. Refactor `selectedText()` into `collectSelected()` + `formatExport()` with three small
   formatters (`asText`, `asMarkdown`, `asJson`) and `fenceFor`.
3. Copy: `copyText(collect().then((items) => formatExport(items, fmt).text))`. Keep the Safari-safe
   promise pattern already in `copyText`.
4. Download: file name `ptrack-jobs-YYYYMMDD-HHMMSS.<ext>`, Blob type from `formatExport`.
   Toast text: `Downloaded 3 jobs (Markdown).`
5. README: a short table of the three formats and what each is good for (Text to read, Markdown to
   paste in an issue or PR, JSON for scripts and for giving to an AI agent).
6. Test: the page contains `id="format"` and the three option values. (The logic is browser-side; the
   manual checks below are the real verification.) If you want logic tests without a browser, keep
   `fenceFor` and `formatExport` free of DOM access so they can be pasted into `node` for a
   one-off check; do not add a JS test runner to the repository.

### Manual checks

- Export the same two jobs in all three formats; open each file. The Markdown renders cleanly on
  GitHub; the JSON parses (`node -e "JSON.parse(require('fs').readFileSync(0,'utf8'))" < file`).
- A job whose output contains ``` and `<script>` and a very long line: Markdown stays valid, nothing
  is interpreted as HTML anywhere.
- A job with no output exports as `(no output)` / `""` in JSON.

**Commit message:** `Dashboard: export selected jobs as Text, Markdown or JSON`

---

## Phase 4: Batch actions

**Goal.** Pause, Resume, Stop and Rerun for all ticked jobs with one click.

### Design decisions

- **No server change.** The dashboard calls the existing `POST /api/jobs/ID/<action>` endpoints,
  one per job, in parallel. The single-writer rule is untouched: those endpoints only write marker
  files / start new jobs, exactly as the per-row buttons do.
- Four buttons in a second row of the bar, each with a count: `Pause (2)`, `Resume (1)`,
  `Stop (3)`, `Rerun (4)`. A button counts only the selected jobs it applies to, and is hidden
  or disabled at 0:
  - Pause → selected jobs with state `running`
  - Resume → state `paused`
  - Stop → state `running` or `paused`
  - Rerun → every other state (finished: `passed`, `failed`, `cancelled`, `lost`)
- Jobs that don't apply are **skipped, not an error**. The result toast says so:
  `Paused 2 jobs, skipped 1 (not running).`
- **Only visible selected jobs** are affected (`shown` ∩ `selected`). Selected jobs hidden by a
  filter are never acted upon. This matters most for Stop and Rerun.
- **Confirmation** (one dialog for the whole batch, same style as `act()`):
  - Stop: `Stop N jobs and everything they started?` followed by the list of ids and titles (first
    10, then `…and K more`).
  - Rerun: `Run N jobs again, each in its own folder?` plus the same list.
  - Pause and Resume: no confirmation (they are reversible, like the single buttons).
- After **Rerun** the selection is cleared (a second click would otherwise start every job again).
  After Pause / Resume / Stop it is kept.
- Failures are collected: `Stopped 2 of 3 jobs. First error: <message>`.

### Steps

1. `src/ui.html`: refactor `act(job, action, button)` into
   `request(job, action)` → `Promise<{ ok, message }>` (does the `fetch`, parses JSON, catches
   network errors) and keep `act()` as the one-job wrapper (confirm, disable button, toast, refresh).
   Behaviour of the per-row buttons must not change.
2. Add `const BATCH = { pause: ..., resume: ..., stop: ..., rerun: ... }`: for each action, a
   predicate over `job.state`, a past-tense word for the toast, and its confirmation text.
3. HTML: a `<div class="bar-actions">` with four buttons (`id="b-pause"`, `b-resume`, `b-stop`,
   `b-rerun`), inside `#bar`, wrapping onto its own row on narrow screens (`flex-wrap`, as the bar
   already does). Style `b-stop` with the existing `danger` class.
4. `updateBar()`: compute the four counts from `shown.filter(j => selected.has(j.id))`, set the
   labels and `hidden`/`disabled`.
5. `async function batch(action)`: gather applicable jobs, `confirm` when needed, run
   `Promise.all(jobs.map((j) => request(j, action)))`, build the toast, clear the selection for
   rerun, then `refresh()`.
6. Disable the four buttons while a batch is running (a module-level `busy` flag) so a double click
   cannot send everything twice.
7. README: a short "Working on several jobs" paragraph listing the four actions and the skip rule.
8. Tests:
   - Page contains `id="b-pause"`, `b-resume`, `b-stop`, `b-rerun`.
   - Server side, extend the dashboard test: start a long-running job (as other tests do, for
     example a sleeping command; look at `pause_and_resume_change_state` and
     `stop_cancels_a_running_job` for the pattern), then call `POST /api/jobs/ID/stop` through the
     test's `http` helper (it currently sends only GET-style requests with no body; a POST with
     `Content-Length: 0` works with the same helper) and assert 200 and a `cancelled` state
     afterwards. This is the endpoint every batch button depends on, so keep it covered.

### Manual checks (throwaway home!)

- Three running jobs + two finished: tick all. Buttons show `Pause (3)`, `Stop (3)`, `Rerun (2)`
  and no Resume.
- Pause → all three go to *paused* within two seconds; `Resume (3)` appears.
- Stop with the confirm → cancelled; Cancel in the confirm → nothing happens.
- Rerun on the two finished jobs → two new jobs appear, selection is cleared.
- Apply a project filter after ticking: the hidden ticked jobs are not touched, and the bar shows
  only visible counts.
- Click a button twice quickly: the actions are sent once.

**Commit message:** `Dashboard: pause, resume, stop or rerun all ticked jobs at once`

---

## Phase 5: Search

**Goal.** A search box that narrows the list as you type, and highlights the matches inside open
logs.

### Design decisions

- **5a: filter the list.** Client-side, over the jobs the server already returned. A job matches when
  every space-separated term (case-insensitive) appears in at least one of: id, title (label or
  command), project, tag, cwd, agent, state, last line.
- **5b: highlight in open logs.** In each open log panel, wrap the matches in `<mark>`. A line
  above the list shows `12 matches in 3 open logs` (a count, not navigation).
- **What is not searched:** the contents of logs that are not open. Searching every log needs a
  server-side scan and is deliberately **left out** of this plan (see "Not in this plan").
- `shown` becomes the **filtered** list, so Select all, Expand all, Copy, Download and the batch
  actions all follow the search without further changes.
- **Selection and search.** Ticks survive a search (clearing the box brings back the ticked
  jobs). Selection is pruned only against the jobs the server returned (`data.jobs`), as before, not
  against the search result. Actions use `shown ∩ selected` (phase 4 rule), and the bar says
  `2 of 5 selected (1 hidden by search)` when they differ.
- Not persisted: the box starts empty on every page load (an old search term is more confusing than
  helpful). `Esc` clears it; `/` focuses it (ignore the key when typing in an input or select).
- Debounce typing by 150 ms.
- **Safety:** matches are inserted as text nodes and `<mark>` elements built with `createElement` /
  `Text`. Never use `innerHTML` with log content or with the search term.

### Steps

1. **Split `refresh()`** into `fetchData()` (the network part and its error handling) and
   `render(data)` (everything that builds the page). Keep the `inFlight` guard on the fetch. Store
   the last result in `lastData` so typing can call `render(lastData)` without a request. This is
   a pure refactor: do it first, run `verify`, and check by hand that the page behaves exactly as
   before (logs keep their scroll, the dropdowns do not close, notifications still fire). You may
   commit it separately as `Dashboard: split refresh into fetch and render` if you want a safe
   checkpoint; if so, keep the phase's main commit message for the rest.
2. HTML: `<input type="search" id="q" placeholder="Search jobs…  ( / )" aria-label="Search jobs">`
   in the header, before the project select. Add `<span id="match-count" class="picked">` in the bar.
3. JS: `function matches(job, terms)`; in `render`, `shown = data.jobs.filter(...)`.
   An empty result with a non-empty search shows a notice: `No jobs match "<term>". Clear the search to see all.` (text via `textContent`; the existing "No jobs" notices stay for the other cases).
4. Counts: the header `counts` text and the tab title keep describing **all** returned jobs, not
   only the filtered ones, so a running job hidden by a search is never forgotten. Add `· N shown`
   only while a search is active.
5. Highlight: `function fillLog(pre, text, terms)` builds the panel content with `Text` and `<mark>`
   nodes; with no terms it just sets `textContent` (fast path). Call it from the places that set a
   panel's text (initial render in `renderJob`, and the fetch callback). Cap at 500 marks per
   panel to keep huge logs responsive; skip empty terms; escape nothing by hand because you
   never build HTML. Add a `mark` style using colours that work in both themes (define variables
   `--mark-bg` and `--mark-text` in both `:root` blocks, as the other colours do).
6. Shortcuts: `keydown` listener for `/` and `Escape`.
7. README + `HELP`: mention the search box, the `/` shortcut and that open logs are highlighted.
8. Tests: the page contains `id="q"`. The logic is browser-side, so describe the manual checks in
   your report.

### Manual checks

- Type a project name, a tag, a job id, a word from a job's last line: the list narrows each time;
  clearing restores it.
- Two terms (`cargo fail`) require both.
- Open a log and search a word in it: it is highlighted; the highlight survives the 1.5 s refresh
  and the log keeps its scroll position.
- Tick three jobs, search so that one is hidden, press Copy: only the visible two are copied and the
  bar says one is hidden.
- A log line that contains `<img src=x onerror=alert(1)>` and a search for `img`: nothing executes,
  the text is shown literally and highlighted.
- `/` focuses the box except while typing in it; `Esc` clears it.

**Commit message:** `Dashboard: search jobs and highlight matches in open logs`

---

## After the last phase

1. Check that README "Dashboard" reads well as a whole (it has grown by five features) and that the
   `ptrack ui` entry in `HELP` still fits in one screen width.
2. Update the roadmap list in the README (a "v0.6" line, as v0.2…v0.5 have).
3. **Do not tag a release on your own.** If the owner asks for v0.6.0: bump `version` in
   `Cargo.toml`, run `verify`, commit, push, wait for green CI, then
   `git tag -a v0.6.0 -m "..."` and `git push origin v0.6.0`, and watch the release workflow.
4. Move this folder to `docs/plan/done/v0.6/` and update `docs/plan/README.md` (same as v0.4).

## Not in this plan (ideas for later)

- **Search inside all logs** (not just the open ones): a server endpoint such as
  `GET /api/search?q=` that scans every log with a total time and size budget.
- A **Download log** button inside each open log panel (the single-job version of phase 2).
- Saving a selection as a named group, or remembering open jobs between page loads.
- Real automated tests for the page's JavaScript (needs a headless browser or a JS runner, which
  the "no new dependencies" rule makes a decision for the owner).
