---
title: "Use acrawl as an MCP server"
description: "Add acrawl's MCP server to Claude Code, Cursor, VS Code, Zed and 13 other clients: 40 browser, DevTools and script tools over stdio."
---

`acrawl mcp` starts a built-in [Model Context Protocol](https://modelcontextprotocol.io) server. It gives any MCP client, such as Claude Code, Cursor, VS Code, Zed, Windsurf, JetBrains IDEs or Gemini CLI, a real browser with DevTools-level observability.

## Install in one command

```bash
acrawl mcp install
```

The installer detects your editors and agents, lets you pick which to configure, and writes the right config file for each, at user or project scope.

Supported clients: Claude Code, Claude Desktop, Cursor, Windsurf, VS Code (Copilot), OpenCode, Zed, TRAE, JetBrains IDEs, Gemini CLI, Qwen Code, Codex CLI, Hermes, OpenClaw, Goose, Crush and Aider.

For Claude Code you can also run:

```bash
claude mcp add acrawl -- acrawl mcp
```

## Configure by hand

Most clients use this shape (Claude Code `.mcp.json`, Cursor `.cursor/mcp.json`, Claude Desktop, Windsurf, TRAE, Gemini CLI, Qwen Code):

```json
{
  "mcpServers": {
    "acrawl": { "command": "acrawl", "args": ["mcp"] }
  }
}
```

VS Code (`.vscode/mcp.json`):

```json
{
  "servers": {
    "acrawl": { "command": "acrawl", "args": ["mcp"] }
  }
}
```

OpenCode (`opencode.json`):

```json
{
  "mcp": {
    "acrawl": { "type": "local", "command": ["acrawl", "mcp"] }
  }
}
```

Zed (`~/.config/zed/settings.json`):

```json
{
  "context_servers": {
    "acrawl": {
      "command": { "path": "acrawl", "args": ["mcp"], "env": {} },
      "settings": {}
    }
  }
}
```

Where each client keeps its config:

| Client | Config file |
|---|---|
| Claude Code | `.mcp.json` (project) or `~/.claude.json` (user) |
| Cursor | `.cursor/mcp.json` |
| Windsurf | `~/.codeium/windsurf/mcp_config.json` |
| Claude Desktop | `%APPDATA%\Claude\claude_desktop_config.json` (Windows), `~/Library/Application Support/Claude/claude_desktop_config.json` (macOS) |
| VS Code (Copilot) | `.vscode/mcp.json` |
| OpenCode | `opencode.json` |
| Zed | `~/.config/zed/settings.json` |
| TRAE | `.trae/mcp.json` |
| Gemini CLI | `~/.gemini/settings.json` |
| Qwen Code | `~/.qwen/settings.json` |

For the other clients, `acrawl mcp install --client <name>` writes the right file; `acrawl mcp install --list-clients` shows the names.

## What the server exposes

The server provides **40 tools** over stdio:

- **32 browser and DevTools tools** for clients that orchestrate step by step: `navigate`, `click`, `fill_form`, `page_map`, `read_content`, `screenshot`, `execute_js`, `list_network_activity`, `inspect_request`, `list_page_logs`, `get_page_performance`, `inspect_cookies`, `inspect_storage`, `measure_coverage`, `audit_accessibility`, `intercept_network` and more. See [Tools](/AgenticCrawler/docs/tools/).
- **7 script tools** for deterministic multi-step runs with no per-step LLM calls: `run_script`, `save_script`, `list_scripts`, `read_script`, `wait_for_scripts`, `script_status`, `cancel_script`.
- **1 autonomous tool**, `run_goal`, which hands a whole task to acrawl's own agent loop.

The browser and script tools share one persistent browser session across calls and need no configuration. `run_goal` builds its own isolated agent and browser, and needs a provider configured with `acrawl auth`.

## Which mode to use

| You want to… | Use |
|---|---|
| Inspect or debug one page, step by step | Direct browser and DevTools tools |
| Repeat the same multi-page extraction cheaply | Script tools |
| Hand off a whole task and get the result back | `run_goal` |

## Drive your logged-in browser from MCP

```bash
acrawl config set browser_backend extension
```

MCP browser tools then go through the acrawl Bridge extension in your own browser. See [Use your real, logged-in browser](/AgenticCrawler/docs/guides/real-browser/).

## Teach your agent when to use which tool

The `acrawl-mcp` agent skill tells a coding agent when to call the direct tools, when to write a script and when to hand off with `run_goal`, and how to debug with the network and console tools. Install it with the MCP config:

```bash
acrawl mcp install --client claude-code --skill
```

Or for any agent the community `skills` CLI supports:

```bash
npx skills add Mingye-Lu/AgenticCrawler
```

## Example requests

Once the server is connected, ask your agent in plain language:

- "Open localhost:5173, log in with the test account, and tell me which requests fail on the dashboard."
- "Check the pricing page on mobile and desktop and take a screenshot of each."
- "Run an accessibility audit on our signup page and fix the critical issues in the code."
- "Use run_goal to collect every job posting title and location from example.com/careers."

## Troubleshooting

| Symptom | Fix |
|---|---|
| The client can't start the server | The client needs `acrawl` on its `PATH`. Use the full path to the binary in `command` if your editor starts with a different environment. |
| A browser window appears when the editor starts | It shouldn't: the browser starts on the first tool call. Update acrawl with `acrawl update`. |
| `run_goal` says no model is configured | Run `acrawl auth <provider>` and `acrawl config set model <provider/model>`. The other tools need no provider. |
| Browser tools fail with a Node.js error | CloakBrowser needs Node.js 20+. Install it, then run `acrawl install-browser`. |
| Extension mode reports another owner | The terminal UI and the MCP server can't share the extension at once. Close the other acrawl process. |

To remove acrawl from a client, run `acrawl mcp uninstall --client <name>`. Add `--skill` to remove the agent skill too.
