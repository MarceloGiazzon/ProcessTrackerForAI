# Phase 4: Webhook notifications

Reference patch: [`patches/phase-4-webhook.patch`](patches/phase-4-webhook.patch)

## Goal

When a job ends, also POST it to a web address the user chose. That covers a chat (Slack,
Discord), a phone (ntfy.sh) or the user's own script. Uses `curl`, which Windows 10+, macOS and
Linux already include.

## What the user sees

```text
$ ptrack notify webhook https://hooks.slack.com/services/...
$ ptrack notify webhook-format text      # plain text instead of JSON (for ntfy)
$ ptrack notify test                     # "delivered a test message to https://..."
$ ptrack notify webhook off
$ ptrack notify webhook file:///etc/passwd
ptrack: the webhook must be an http:// or https:// address, got 'file:///etc/passwd'
```

Payload (`json` format, the default):

```json
{ "text": "Job failed: npm test\ntook 3m04s · exit 1\nnpm ERR! ...",
  "content": "<same as text>",
  "job": { ...the same object as `ptrack status --json`... } }
```

Slack reads `text`, Discord reads `content`, scripts read `job`. The `text` format sends just the
two lines.

## Design

- `NotifyConfig` gains `webhook: Option<String>` and `webhook_format: WebhookFormat`. The new enum
  is serialized as `"json"` / `"text"` and defaults to `json`. Because of `#[serde(default)]`,
  settings files written by phase 3 still load.
- `job_finished` sends the webhook after the desktop notification, with the same
  `min_seconds` / cancelled rules. Never wait for it.
- `send_webhook(url, content_type, payload, wait)` runs:
  ```text
  curl --silent --show-error --fail --max-time 10 --request POST
       --header "Content-Type: ..." --header "Expect:" --data-binary @- --url <URL>
  ```
  and writes the payload to curl's **stdin**. Why each part matters:
  - `--url` plus the http/https check: a URL can never be read as a curl option or a local file.
  - `--header "Expect:"`: without it, curl waits up to 1 s for a "100 Continue" reply on payloads
    over 1 KB.
  - `--max-time 10`: a dead server never leaves curl running forever.
  - stdin, not the command line: payloads can be long and contain any character.
- On Windows, use `%SystemRoot%\System32\curl.exe` when it exists, and `CREATE_NO_WINDOW`.
- `ptrack notify test` sends a test **webhook** when one is set. It sends a test **desktop**
  notification when desktop notifications are on, or when no webhook is set. It waits for the
  webhook's result and reports it.

## Steps

1. **`src/config.rs`**: add the two fields to `NotifyConfig` and its `Default`. Add
   `enum WebhookFormat { #[default] Json, Text }` with
   `#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]` and
   `#[serde(rename_all = "lowercase")]`.
2. **`src/notify.rs`** (copy from the patch):
   - Imports: `std::io::Write`, `std::path::PathBuf`, `serde_json::json`, `WebhookFormat`.
   - In `job_finished`: after the desktop block, add the `if let Some(url) = &cfg.webhook { ... }`
     block.
   - New functions: `webhook_payload`, `valid_webhook_url`, `curl_program`, `send_webhook`.
   - `command`: new arms `["webhook", "off"]`, `["webhook", url]` (validated),
     `["webhook-format", "json" | "text"]`; `["test"]` now passes `&cfg`. Add the `USAGE`
     constant.
   - `show`: print the webhook line.
   - `send_test(cfg)`: build a sample finished job; desktop and/or webhook as described above.
3. **`src/main.rs`**: two more `HELP` lines (see the patch).
4. **`tests/cli.rs`**: add the helper `fake_webhook(respond: bool)`, a one-request HTTP server
   running in a thread, and three tests:
   - `webhook_receives_finished_jobs`: checks the JSON payload contents.
   - `webhook_test_command_reports_delivery_and_bad_urls_are_refused`: `text` format, `notify
     test`, and 4 bad URLs that must exit with code 2.
   - `a_hanging_webhook_does_not_delay_the_command`: **the regression test for phase 3's pipe
     fix.** The fake server never answers, and `ptrack run` with captured output must still
     finish in under 5 s.
5. **`README.md`**: the webhook paragraph in the Notifications section, and a Commands table row
   (see the patch).

## Prove the regression test works (do this once)

A test that never fails is worthless. Check that this one catches the bug it is meant for:

1. In `src/notify.rs`, temporarily comment out the line
   `crate::platform::stop_sharing_std_handles();`.
2. Run `cargo test --test cli a_hanging`. On Windows it must **FAIL** with
   "took 10.…s: the notifier kept the caller's pipe open".
3. Put the line back. Run the test again: it passes.

(This was done while writing the plan: 10.26 s without the fix.)

## Manual checks

```sh
# A free public test endpoint; open the printed page in a browser to see the request arrive.
ptrack notify webhook https://ntfy.sh/ptrack-test-$RANDOM   # or any webhook.site URL
ptrack notify webhook-format text
ptrack notify test
ptrack notify webhook off
```

Use a throwaway `PTRACK_HOME`. Never set a webhook in the owner's real settings.

## Pitfalls

- Never build the curl command as one string for a shell. Use `Command` with separate arguments,
  as in the patch.
- Don't add a JSON or HTTP library: `serde_json` and `curl` are enough.
- Webhook payloads contain the command line and last output line. That is documented in the
  README; don't add more data to the payload.

## Commit message

```text
Webhook notifications when a job ends (ptrack notify webhook)
```

## Done when

- [ ] `verify` passes; CI is green (the webhook tests also run on Linux and macOS)
- [ ] The regression test was seen failing without the fix, and passing with it
