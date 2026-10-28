---
title: "Install acrawl"
description: "Install acrawl on Linux, macOS or Windows with one command. The script verifies the release checksum and sets up the stealth browser."
head:
  - tag: title
    content: "Install acrawl on Linux, macOS and Windows"
  - tag: meta
    attrs:
      property: og:title
      content: "Install acrawl on Linux, macOS and Windows"
---

acrawl installs as a single binary. The install script downloads the latest release for your platform, verifies its SHA256 checksum and sets up CloakBrowser, the stealth browser acrawl uses for JavaScript-heavy pages.

## Linux and macOS (x64, ARM64)

```bash
curl -fsSL https://raw.githubusercontent.com/Mingye-Lu/AgenticCrawler/main/install.sh | bash
```

## Windows (x64, PowerShell)

```powershell
irm https://raw.githubusercontent.com/Mingye-Lu/AgenticCrawler/main/install.ps1 | iex
```

## Requirements

- **Node.js 20+** for browser features. Plain HTTP fetching works without it.
- **An API key** for at least one of the [25 supported LLM providers](https://github.com/Mingye-Lu/AgenticCrawler#25-llm-providers), or a local OpenAI-compatible endpoint. The MCP server's 39 direct and script tools work without any key; only the autonomous `run_goal` tool needs one.

acrawl checks for updates on startup and tells you when a new version is available.

## Build from source

```bash
git clone https://github.com/Mingye-Lu/AgenticCrawler.git
cd AgenticCrawler
cargo build --release
npm install   # CloakBrowser bridge for browser automation
```

The binary is written to `target/release/acrawl`.

## Optional: the browser extension

The **acrawl Bridge** extension lets acrawl drive your real browser, with your existing sessions and cookies, instead of a headless one. See [Use your real, logged-in browser](/AgenticCrawler/docs/guides/real-browser/).

## Optional: the agent skill

If you drive acrawl from a coding agent, install the `acrawl-mcp` skill so the agent knows when to use direct tools, scripts or `run_goal`:

```bash
npx skills add Mingye-Lu/AgenticCrawler
```

Or configure the MCP server and install the skill in one step, without Node:

```bash
acrawl mcp install --client claude-code --skill
```

Next: [Quick start](/AgenticCrawler/docs/quickstart/).
