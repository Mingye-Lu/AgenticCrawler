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
- **An API key** for at least one of the [25 supported LLM providers](/AgenticCrawler/docs/providers/), or a local OpenAI-compatible endpoint. The MCP server's 39 direct and script tools work without any key; only the autonomous `run_goal` tool needs one.

## What the script does

1. Detects your operating system and CPU, and picks the matching release (Linux and macOS on x64 or ARM64, Windows on x64).
2. Downloads it from the latest GitHub release and checks it against the published SHA256 checksum. A mismatch stops the install.
3. On Linux and macOS, puts the binary in `~/.local/bin` (or `ACRAWL_INSTALL_DIR`) and tells you if that folder isn't on your `PATH`. On Windows, puts it in `%USERPROFILE%\.acrawl\bin` and adds that folder to your user `PATH`.
4. Installs CloakBrowser into `~/.acrawl` if Node.js 20+ is available. The browser itself downloads the first time a page needs it.

Prefer to read the script first? It's [install.sh](https://github.com/Mingye-Lu/AgenticCrawler/blob/main/install.sh) or [install.ps1](https://github.com/Mingye-Lu/AgenticCrawler/blob/main/install.ps1). You can also download a binary and `checksums.sha256` from the [releases page](https://github.com/Mingye-Lu/AgenticCrawler/releases) yourself.

## Check that it works

```bash
acrawl --version
acrawl auth list
```

The first prints the version and build details; the second lists the providers you can set up. Neither needs a key or a browser.

## Update and uninstall

acrawl checks for updates on startup and tells you when a new version is available. To install it:

```bash
acrawl update
```

To remove acrawl, keeping your settings and credentials:

```bash
acrawl uninstall
```

Add `--purge` to delete `~/.acrawl` as well, including credentials and saved sessions.

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

## Troubleshooting

| Problem | Fix |
|---|---|
| `acrawl: command not found` | The install folder isn't on your `PATH`. On Linux and macOS the script prints the line to add to your shell profile. On any system, open a new terminal afterwards. |
| "Node.js 20+ is required for browser automation" | Install Node.js 20 or newer, then run `acrawl install-browser`. Pages that don't need JavaScript still work without it. |
| CloakBrowser install failed | Run `acrawl install-browser`, or `npm install --prefix ~/.acrawl cloakbrowser playwright-core`. |
| `acrawl` refuses to start the UI | The terminal UI needs an interactive terminal. In scripts and CI, use `acrawl prompt` or `acrawl run`. |

Next: [Quick start](/AgenticCrawler/docs/quickstart/).
