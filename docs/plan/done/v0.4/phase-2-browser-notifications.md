# Phase 2: Browser notifications (dashboard)

Reference patch: [`patches/phase-2-browser-notifications.patch`](patches/phase-2-browser-notifications.patch)

## Goal

While `ptrack ui` is open, the user can get a browser notification whenever a job finishes.
Only `src/ui.html` changes (plus one test line and the README). No Rust code changes.

## What the user sees

- A new checkbox in the dashboard header: **Notify when jobs end**.
- The first time it is ticked, the browser asks for permission. If the user refuses, the box is
  unticked again and a message explains that notifications are blocked.
- From then on, when a job the page saw **running or paused** becomes passed, failed, cancelled
  or lost, a notification appears: title `"<label or command>: <state>"`, body
  `"took 1m02s · exit 1"` plus the last output line. Clicking it focuses the page and opens that
  job's log.
- The choice is remembered in the browser (`localStorage` key `ptrack.notify`).

## Design

- Notifications are compared **between two refreshes** of the job list, which happen every 1.5 s:
  a `Map` from job id to the last state seen.
- **Nothing is notified on the first load**, or jobs that ended before the page opened would all
  pop up at once (`firstLoad` flag).
- `Notification.requestPermission()` is called only from the checkbox's `change` event. Browsers
  ignore permission requests that don't come from a user action.
- `http://127.0.0.1` counts as a secure origin, so the Notification API works without HTTPS.
- `tag: "ptrack-" + id` stops duplicate pop-ups for the same job.

## Steps

1. **Header**: after the "Show older jobs" `<label>`, add:
   ```html
   <label class="toggle" title="Show a notification on this computer when a job finishes (while this page is open)"><input type="checkbox" id="notify"> Notify when jobs end</label>
   ```
2. **In `refresh()`**, right after `const data = await r.json();`, add `notifyFinished(data.jobs);`.
3. **Before the final `refresh();` call** at the bottom of the script, add the block from the
   patch: `seenStates`, `firstLoad`, `notifyOn`, `canNotify`, the checkbox `change` handler, and
   `function notifyFinished(jobs)`.
4. **`tests/cli.rs`**, in `dashboard_requires_token_and_local_host`, after the `<title>`
   assertion:
   ```rust
   assert!(page.contains("id=\"notify\""), "notification toggle is on the page");
   ```
5. **`README.md`**: in the Dashboard section, after the paragraph ending with "`--no-open`.", add
   the paragraph about **Notify when jobs end** and the **Rerun** button (see the patch).

## Check the JavaScript before running anything

A typo in the page breaks the whole dashboard, and no Rust test catches that. Node.js is
installed on the owner's machine. Extract the script and syntax-check it:

```sh
python -c "s=open('src/ui.html',encoding='utf-8').read(); open('ui-check.js','w',encoding='utf-8').write(s.split('<script>')[1].split('</script>')[0])"
node --check ui-check.js && echo "JS syntax OK"
rm ui-check.js
```

## Manual checks

1. `cargo build`, then `./target/debug/ptrack ui` with a throwaway `PTRACK_HOME`.
2. Tick **Notify when jobs end** and allow notifications.
3. In another terminal (same `PTRACK_HOME`): `ptrack run -d -- ping -n 5 127.0.0.1` on Windows,
   or `ptrack run -d -- sleep 5` on Linux/macOS.
4. About 5 seconds later, a notification "ping -n 5 127.0.0.1: passed" appears. Clicking it opens
   that job's log in the page.
5. Reload the page: no notifications appear for jobs that had already ended.

## Pitfalls

- Use `title(j)` and `dur(...)`, which already exist in the page. Don't write new formatting.
- Never put job text into `innerHTML`. The Notification API takes plain strings, which is safe.

## Commit message

```text
Dashboard: browser notifications when jobs end
```

## Done when

- [ ] `node --check` passes, `verify` passes, CI is green
- [ ] The manual checks behave as described
