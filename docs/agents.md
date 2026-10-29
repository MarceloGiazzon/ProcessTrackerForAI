# Setting up ptrack with your AI agent

ptrack works with any agent that can run shell commands. For each agent below there are up to three
steps. Do the first one at least; the others make it better.

1. **Instructions:** tell the agent to run long commands through ptrack.
2. **MCP:** let the agent check on jobs with one small tool call instead of reading logs.
3. **Attribution:** make `ptrack list` show which agent (and session) started each job.

`ptrack instructions` prints the instructions block. Append it to the file your agent reads:

```sh
ptrack instructions >> AGENTS.md                 # Git Bash, Linux, macOS
ptrack instructions | Add-Content AGENTS.md      # PowerShell
```

The block is in English on purpose: agents follow it most reliably that way.

| Agent | Instructions file | MCP | Detected automatically |
|---|---|---|---|
| Claude Code | `CLAUDE.md` | `claude mcp add` | ✅ agent and session (plus the [hook](../README.md#automatic-tracking-in-claude-code-hook)) |
| Codex | `AGENTS.md` | `codex mcp add` | ✅ agent and session |
| Cursor | `AGENTS.md` or `.cursor/rules/` | `.cursor/mcp.json` | ✅ agent |
| Antigravity CLI | `AGENTS.md` (`GEMINI.md` still works) | `mcp_config.json` | ✅ agent |
| Gemini CLI | `GEMINI.md` | `gemini mcp add` | ✅ agent |
| Aider | `CONVENTIONS.md` | not supported | set `PTRACK_AGENT=aider` |
| Anything else | its own rules file | its own MCP config | set `PTRACK_AGENT` / `PTRACK_SESSION` |

Every MCP example below is read-only. Add `--allow-control` to the arguments (`["mcp",
"--allow-control"]`) if you want the agent to stop, pause, resume and rerun jobs too.

## Codex (OpenAI)

```sh
ptrack instructions >> AGENTS.md
codex mcp add ptrack -- ptrack mcp
```

Or add this to `~/.codex/config.toml` yourself:

```toml
[mcp_servers.ptrack]
command = "ptrack"
args = ["mcp"]
```

ptrack recognises Codex by the variables it sets for the commands it runs (`CODEX_THREAD_ID`,
`CODEX_SANDBOX`, `CODEX_CI`) and uses the thread id as the session. If your
`shell_environment_policy` in `config.toml` only lets through some variables, add
`CODEX_THREAD_ID` to that list.

**Sandbox:** in Codex's default sandbox, commands may not be allowed to write outside the project.
ptrack keeps its data in a per-user folder (see [README](../README.md#how-it-works)), so either
allow that folder, or keep ptrack's data inside the project with `PTRACK_HOME=.ptrack` (add
`.ptrack/` to `.gitignore`).

## Cursor

```sh
ptrack instructions >> AGENTS.md
```

If you prefer Cursor's own rules, save the same text as `.cursor/rules/ptrack.mdc` instead.

MCP: create `.cursor/mcp.json` in the project (or `~/.cursor/mcp.json` for every project):

```json
{ "mcpServers": { "ptrack": { "type": "stdio", "command": "ptrack", "args": ["mcp"] } } }
```

ptrack recognises Cursor's agent by `CURSOR_AGENT`. There is no known Cursor session variable, so
jobs show the agent only. Set `PTRACK_SESSION` yourself if you need sessions.

## Antigravity CLI (`agy`) and Gemini CLI

Google replaced Gemini CLI with Antigravity CLI in June 2026. Gemini CLI still works for some
enterprise accounts. The setup is almost the same.

**Antigravity CLI:**

```sh
ptrack instructions >> AGENTS.md
```

MCP: add ptrack to `~/.gemini/config/mcp_config.json` (every project) or `.agents/mcp_config.json`
(this project):

```json
{ "mcpServers": { "ptrack": { "command": "ptrack", "args": ["mcp"] } } }
```

**Gemini CLI:**

```sh
ptrack instructions >> GEMINI.md
gemini mcp add --scope user ptrack ptrack mcp
```

ptrack recognises them by `ANTIGRAVITY_AGENT` and `GEMINI_CLI`.

## Aider

```sh
ptrack instructions >> CONVENTIONS.md
```

Then load that file every time, by adding this line to `.aider.conf.yml`:

```yaml
read: CONVENTIONS.md
```

Aider has no MCP support and doesn't identify itself to the commands it runs. To see `aider` in
the AGENT column, start it with the variable set:

```sh
PTRACK_AGENT=aider aider                        # Git Bash, Linux, macOS
$env:PTRACK_AGENT = "aider"; aider              # PowerShell
```

## Any other agent

1. Paste the output of `ptrack instructions` into the agent's rules file.
2. If it supports MCP, register a stdio server with command `ptrack` and arguments `["mcp"]`.
3. If it sets `AI_AGENT` for the commands it runs, ptrack already shows its name. Otherwise start
   the agent with `PTRACK_AGENT=<name>` (and `PTRACK_SESSION=<id>` if you want sessions).

## Checking that it works

Ask the agent to run something slow, for example "run the test suite". Then:

```sh
ptrack list
```

The job should appear with the agent's name in the AGENT column and your project in the PROJECT
column. If AGENT shows `-`, the agent didn't identify itself: use `PTRACK_AGENT` as shown above.
