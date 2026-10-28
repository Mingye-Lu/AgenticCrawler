<p align="center">
<pre align="center">
  █████╗  ██████╗██████╗  █████╗ ██╗    ██╗██╗     
 ██╔══██╗██╔════╝██╔══██╗██╔══██╗██║    ██║██║     
 ███████║██║     ██████╔╝███████║██║ █╗ ██║██║     
 ██╔══██║██║     ██╔══██╗██╔══██║██║███╗██║██║     
 ██║  ██║╚██████╗██║  ██║██║  ██║╚███╔███╔╝███████╗
 ╚═╝  ╚═╝ ╚═════╝╚═╝  ╚═╝╚═╝  ╚═╝ ╚══╝╚══╝ ╚══════╝
</pre>
</p>

<p align="center">
  <strong>Browser automation agent that acts and observes.</strong> Navigate, click, and extract — or inspect network traffic, debug console errors, profile performance, and audit accessibility. One Rust binary.
</p>

<p align="center">
  <a href="https://github.com/Mingye-Lu/AgenticCrawler/actions/workflows/ci.yml"><img src="https://github.com/Mingye-Lu/AgenticCrawler/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/rust-2021_edition-orange.svg" alt="Rust"></a>
  <a href="https://mingye-lu.github.io/AgenticCrawler/"><img src="https://img.shields.io/badge/website-acrawl-c8f43c.svg" alt="Website"></a>
</p>

<p align="center">
  <a href="https://mingye-lu.github.io/AgenticCrawler/"><strong>Website</strong></a> ·
  <a href="https://mingye-lu.github.io/AgenticCrawler/docs/">Docs</a> ·
  <a href="https://mingye-lu.github.io/AgenticCrawler/docs/install/">Install</a> ·
  <a href="https://mingye-lu.github.io/AgenticCrawler/docs/quickstart/">Quick start</a> ·
  <a href="https://mingye-lu.github.io/AgenticCrawler/docs/mcp-server/">MCP server</a> ·
  <a href="https://mingye-lu.github.io/AgenticCrawler/docs/comparisons/overview/">Comparisons</a>
</p>


```text
acrawl > get the name, price and rating of every book on books.toscrape.com, all 50 pages
```

acrawl reads the goal, opens the site, works out the pagination and hands back JSON. Along the way the agent can watch the network, read the console, profile performance and audit accessibility, so it can debug a page as well as scrape it. It also runs as an MCP server, so Claude Code, Cursor, VS Code and 14 other clients can drive the same browser.

## Install

**Linux / macOS (x64, ARM64)**

```bash
curl -fsSL https://raw.githubusercontent.com/Mingye-Lu/AgenticCrawler/main/install.sh | bash
```

**Windows (x64, PowerShell)**

```powershell
irm https://raw.githubusercontent.com/Mingye-Lu/AgenticCrawler/main/install.ps1 | iex
```

The script downloads the latest release, verifies its SHA256 checksum and sets up CloakBrowser, the stealth browser acrawl uses for JavaScript-heavy pages. Browser features need Node.js 20+. To build from source, see the [install guide](https://mingye-lu.github.io/AgenticCrawler/docs/install/#build-from-source).

## Quick start

```bash
acrawl auth anthropic          # or openai, google, other (Ollama, LM Studio, ...), 25 providers in all
acrawl                         # interactive terminal UI
acrawl prompt "list the titles of the top 10 stories on news.ycombinator.com"   # one goal, then exit
```

Then go further:

```bash
acrawl mcp install                     # add acrawl to Claude Code, Cursor, VS Code and other MCP clients
acrawl run books_catalog -o books.json # replay a saved script with no LLM calls
```

The [quick start](https://mingye-lu.github.io/AgenticCrawler/docs/quickstart/) walks through a first crawl, getting JSON out, and writing goals that work.

## What you can do with it

**Extract data from any site.** Describe the fields you want; acrawl fetches static pages over HTTP and switches to a real browser only when a page needs JavaScript.

```text
acrawl > get all repository names and star counts from github.com/trending
```

**Debug a page like a developer would.** Network requests, console logs, WebSocket frames, performance timings, cookies, storage, code coverage and axe-core accessibility audits are all agent tools.

```text
acrawl > open localhost:3000/checkout, submit the form, and tell me which requests failed and why
```

**Fill forms and work through flows.** Click, type, select, hover, press keys and emulate phones. With the [acrawl Bridge extension](https://chromewebstore.google.com/detail/acrawl-bridge/nbkpmfofkdjcgkncmpicbdkjmddllcep), it works inside your own logged-in Chrome, Edge or Brave.

**Run it again for free.** Save a crawl as a deterministic JSON script with loops, conditions and parallel branches, then replay it from cron or CI with `acrawl run`, with no model calls.

**Split big jobs.** The agent forks sub-agents onto separate tabs, and a URL-claiming registry stops two of them from crawling the same page.

## Why acrawl

- **One binary.** No Python environment, no Docker, no hosted service. Node.js is needed only for the bundled browser.
- **Acts and observes.** Most browser agents navigate and click. acrawl's agent also sees the network, the console and performance, as first-class tools.
- **AI where you need it, scripts where you don't.** Let the agent explore once; replay the result deterministically.
- **Your choice of model.** 25 providers, including Anthropic, OpenAI, Gemini, DeepSeek, Bedrock, Azure, Vertex AI, GitHub Copilot, Groq, Mistral and OpenRouter, plus any OpenAI-compatible endpoint such as Ollama.
- **MCP both ways.** Use other MCP servers as extra agent tools, or expose acrawl's 40 tools to your coding agent with `acrawl mcp`.
- **Open source.** MIT-licensed, runs on your machine, with your own LLM key and no telemetry.

## How it compares

| | acrawl | browser-use | Playwright MCP | Firecrawl |
|---|---|---|---|---|
| What it is | Browser agent, CLI and MCP server | Python agent library and hosted agent | MCP server for browser control | Hosted scrape and crawl API |
| Own reasoning loop | Yes, local | Yes, local or hosted | No, your client decides each step | Yes, hosted |
| Install | Single Rust binary | Python package | Node package | Hosted, or self-host |
| DevTools tools for the agent | Network, console, WebSocket, performance, cookies, storage, coverage, axe-core | Not a focus | Console, network | Not a focus |
| Replays with no LLM calls | Yes, JSON scripts | No | No | No |
| License | MIT | MIT | Apache 2.0 | AGPL-3.0 engine |

Reviewed October 2026. The [full comparison](https://mingye-lu.github.io/AgenticCrawler/docs/comparisons/overview/) has a page for each tool.

## Documentation

Everything else lives on the website, **[mingye-lu.github.io/AgenticCrawler](https://mingye-lu.github.io/AgenticCrawler/)**.

| | |
|---|---|
| **Get started** | [Install](https://mingye-lu.github.io/AgenticCrawler/docs/install/) · [Quick start](https://mingye-lu.github.io/AgenticCrawler/docs/quickstart/) · [LLM providers](https://mingye-lu.github.io/AgenticCrawler/docs/providers/) · [MCP server](https://mingye-lu.github.io/AgenticCrawler/docs/mcp-server/) |
| **Guides** | [JavaScript-heavy sites](https://mingye-lu.github.io/AgenticCrawler/docs/guides/scrape-js-sites/) · [Deterministic scripts](https://mingye-lu.github.io/AgenticCrawler/docs/guides/deterministic-scripts/) · [Debugging with DevTools](https://mingye-lu.github.io/AgenticCrawler/docs/guides/devtools-debugging/) · [Your own browser](https://mingye-lu.github.io/AgenticCrawler/docs/guides/real-browser/) |
| **Reference** | [How it works](https://mingye-lu.github.io/AgenticCrawler/docs/how-it-works/) · [All 43 tools](https://mingye-lu.github.io/AgenticCrawler/docs/tools/) · [CLI and slash commands](https://mingye-lu.github.io/AgenticCrawler/docs/cli/) · [Configuration](https://mingye-lu.github.io/AgenticCrawler/docs/configuration/) · [FAQ](https://mingye-lu.github.io/AgenticCrawler/docs/faq/) |

Driving acrawl from a coding agent? `npx skills add Mingye-Lu/AgenticCrawler` installs the `acrawl-mcp` skill, which teaches the agent when to use direct tools, scripts or `run_goal`.

## Development

acrawl is a Rust workspace of 11 crates.

```bash
cargo build --release
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

<p align="center">
  <img src="docs/assets/architecture.svg" alt="acrawl architecture: the TUI, one-shot prompt, and MCP entry points feed a runtime turn loop that streams from an LLM provider and runs 43 tools, which fetch over HTTP, escalate to CloakBrowser, or drive the user's Chrome through a WebSocket extension bridge." width="100%">
</p>

See [CONTRIBUTING.md](CONTRIBUTING.md) to get started, [CHANGELOG.md](CHANGELOG.md) for release notes and [SECURITY.md](SECURITY.md) to report a vulnerability.

## License

[MIT](LICENSE)
