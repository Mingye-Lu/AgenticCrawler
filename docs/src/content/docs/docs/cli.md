---
title: "Command-line reference"
description: "Every acrawl command, flag, slash command, keybinding and exit code, for the terminal UI, one-shot prompts, saved scripts, CI and coding agents."
head:
  - tag: title
    content: "acrawl CLI reference: commands, flags and slash commands"
  - tag: meta
    attrs:
      property: og:title
      content: "acrawl CLI reference: commands, flags and slash commands"
---

**Short answer:** `acrawl` on its own opens the terminal UI. `acrawl prompt "<goal>"` runs one goal and exits. `acrawl run <script>` replays a saved script with no LLM. `acrawl mcp` starts the MCP server. Everything else, from `auth` and `config` to `update`, configures those four. `acrawl --help` prints the same reference from your installed version.

## Commands

| Command | What it does |
|---|---|
| `acrawl` | Start the interactive terminal UI. Needs a terminal; piped output is refused with a pointer to `prompt`. |
| `acrawl prompt <goal>` | Run one goal and exit. `acrawl "<goal>"` is a shorthand. |
| `acrawl --resume <session.json> [/cmd ...]` | Load a saved session and run resume-safe slash commands on it, without opening the UI. |
| `acrawl run <name or file.json>` | Replay a [deterministic script](/AgenticCrawler/docs/guides/deterministic-scripts/). No model, no credentials. |
| `acrawl mcp` | Start the [MCP server](/AgenticCrawler/docs/mcp-server/) over stdio. |
| `acrawl mcp install` / `uninstall` | Add or remove acrawl in your editors' MCP config. |
| `acrawl auth <provider>` | Set up a [provider](/AgenticCrawler/docs/providers/). Also `auth status` and `auth list`. |
| `acrawl config get / set / unset / path` | Read and write [settings](/AgenticCrawler/docs/configuration/). |
| `acrawl install-browser` | Install CloakBrowser and download its browser binary. |
| `acrawl update` | Replace the binary with the latest release. |
| `acrawl uninstall [--purge]` | Remove acrawl. `--purge` also deletes settings, credentials and sessions. |
| `acrawl system-prompt` | Print the agent's system prompt, for debugging. |

## Global flags

| Flag | What it does |
|---|---|
| `--model <provider/model>` | Use this model for this run |
| `--output-format text\|json` | Output format for `prompt` and `--resume` |
| `--headed`, `--no-headless` | Show the browser window |
| `--headless[=true\|false]` | Force headless on or off |
| `--allowedTools <a,b,...>` | Only let the agent use these tools. Repeatable. |
| `-V`, `--version` | Print version and build details |

`--allowedTools` is the way to lock a run down. For example, to let the agent read but never click or fill:

```bash
acrawl prompt "list the article titles on example.com" \
  --allowedTools navigate,read_content,page_map,screenshot
```

## `acrawl run` flags

| Flag | What it does |
|---|---|
| `--format json\|jsonl\|result` | `json` (default) prints the collected data as one array; `jsonl` prints one item per line; `result` prints the full envelope with status, error and data |
| `--output <path>`, `-o` | Write data to a file instead of stdout |
| `--flatten` | Unwrap one level of arrays, so one `collect` per page becomes one record per item |
| `--max-steps N`, `--timeout SECS`, `--step-timeout SECS` | Tighter limits for this run |
| `--quiet` | No progress on stderr |

```bash
acrawl run mideast_headlines --format jsonl --flatten --output headlines.jsonl
```

## `acrawl auth`

```bash
acrawl auth anthropic                              # interactive
acrawl auth openai --api-key "sk-..."              # non-interactive
acrawl auth amazon-bedrock --access-key AKIA... --secret-key ... --region us-east-1
acrawl auth azure --resource-name r --deployment-name gpt-4o --api-key ...
acrawl auth other --base-url http://localhost:11434/v1
acrawl auth status [--check <provider>] [--json]
acrawl auth list [--json]
```

## `acrawl mcp install`

```bash
acrawl mcp install                                   # pick editors interactively
acrawl mcp install --client claude-code,cursor       # specific editors
acrawl mcp install --all --yes                       # every detected editor
acrawl mcp install --client opencode --scope project # project instead of user config
acrawl mcp install --client claude-code --skill      # also install the agent skill
acrawl mcp install --list-clients
```

## Slash commands

Type `/` in the terminal UI to see these with tab completion. Commands marked resume-safe also work on the command line after `--resume`.

| Command | What it does | Resume-safe |
|---|---|:---:|
| `/help` | List commands | ✓ |
| `/status` | Model, tokens and cost for this session | ✓ |
| `/cost` | Token use and cost in detail | ✓ |
| `/compact` | Compact the conversation now | ✓ |
| `/clear` | Start a fresh session | ✓ |
| `/export [file]` | Save the conversation as Markdown | ✓ |
| `/config [model]` | Show configuration | ✓ |
| `/version` | Version and build details | ✓ |
| `/model [name]` | Show or switch the model | |
| `/sessions` | Pick a saved session | |
| `/auth [provider]` | Set up a provider | |
| `/headed`, `/headless` | Show or hide the browser | |
| `/extension [stop]` | Use your own browser through the extension, or stop | |
| `/cloakbrowser` | Go back to the bundled browser | |
| `/debug` | Show the raw input and output of the last browser tool call | |
| `/exit` | Save and quit | |

Sessions save automatically when you quit. To tidy one up from a script:

```bash
acrawl --resume session.json /compact /export notes.md
```

## Keyboard shortcuts

| Key | Action |
|---|---|
| `Enter` | Send |
| `Shift+Enter`, `Ctrl+J` | New line |
| `Tab` | Complete a slash command |
| `PageUp`, `PageDown` | Scroll the transcript |
| `Ctrl+T` | Cycle reasoning effort (high, medium, low) |
| `Esc` `Esc` | Interrupt the running task |
| `Ctrl+C` | Interrupt when busy, quit when idle |

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Success |
| `1` | Runtime or I/O error |
| `2` | Usage or configuration error, including an invalid script |
| `3` | A state check failed, such as `auth status --check` for an unconfigured provider |
| `130` | `acrawl run` was interrupted; data collected so far is still written |

## Using acrawl from CI or another agent

Everything the terminal UI does has a non-interactive form, so a CI job or coding agent can set acrawl up and check it is ready before relying on it:

```bash
acrawl auth anthropic --api-key "$ANTHROPIC_API_KEY"
acrawl config set model anthropic/claude-sonnet-4-6
acrawl auth status --check anthropic || exit 1
acrawl prompt "collect the release notes from example.com/changelog" --output-format json
```
