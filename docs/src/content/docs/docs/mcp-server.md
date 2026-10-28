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

See the [README](https://github.com/Mingye-Lu/AgenticCrawler#manual-configuration) for OpenCode and Zed.

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
