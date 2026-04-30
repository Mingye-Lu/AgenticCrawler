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
  <strong>LLM-powered web crawler.</strong> Describe what you want in plain English — get structured data back.
</p>

<p align="center">
  <a href="https://github.com/Mingye-Lu/AgenticCrawler/actions/workflows/ci.yml"><img src="https://github.com/Mingye-Lu/AgenticCrawler/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/rust-2021_edition-orange.svg" alt="Rust"></a>
</p>

<p align="center">
  Single binary. No Python runtime. 19 tools. 24 LLM providers. Playwright for the hard parts.
</p>

---

## Why acrawl?

Most web scraping still means writing code: XPath selectors, pagination logic, retry handling, anti-bot workarounds. LLMs can read pages like humans do, but wiring one up to a browser is a project in itself.

acrawl is that wiring, packaged as a single Rust binary. You describe a goal; the agent figures out which pages to visit, what to click, what to extract, and when it's done.

- **No code required.** Describe the goal in English. The agent plans and executes.
- **One binary, zero runtimes.** `cargo build --release` produces a self-contained executable. No Python, no Node runtime — just Rust and a Chromium download for browser automation.
- **Smart fetching.** Static pages are served over HTTP (fast). When JavaScript or interaction is needed, acrawl detects JS framework markers (`__next_data__`, `__nuxt`, `__vue`, `ng-app`, React roots), auth redirects, and short `<noscript>` bodies — then transparently escalates to a headless browser.
- **19 tools, not a chatbot.** The agent has real tools — navigate, click, fill forms, run JS, take screenshots, manage tabs — plus a fork/join layer to spawn parallel sub-agents across multiple browser tabs.
- **24 LLM providers.** Anthropic, OpenAI, Google Gemini, AWS Bedrock, Azure OpenAI, Vertex AI, GitHub Copilot, Groq, Mistral, xAI, Cohere, Alibaba DashScope, OpenRouter, and more. Or bring your own via any OpenAI-compatible endpoint.
- **MCP support.** Extend the agent with custom tools via [Model Context Protocol](https://modelcontextprotocol.io) servers (stdio, SSE, HTTP, WebSocket).

### How does it compare?

| | acrawl | Scrapy | Playwright scripts | Browser-use |
|---|:---:|:---:|:---:|:---:|
| No code needed | Yes | No | No | Yes |
| Single binary | Yes | No | No | No |
| JS rendering | Yes | No | Yes | Yes |
| LLM-powered | Yes | No | No | Yes |
| No Python required | Yes | No | No | No |
| Form filling / interaction | Yes | Limited | Yes | Yes |
| Sub-agent parallelism | Yes | N/A | No | No |
| 24 provider support | Yes | N/A | N/A | Limited |
| MCP extensibility | Yes | No | No | No |

## Quick Start

### Install

```bash
# Build from source
git clone https://github.com/Mingye-Lu/AgenticCrawler.git
cd AgenticCrawler
cargo build --release

# Install Playwright's Chromium (required for browser automation)
npm install
npx playwright install chromium
```

### Configure

```bash
# Set up your LLM provider (interactive prompt)
./target/release/acrawl auth anthropic   # or: openai, other
```

Credentials are stored in `~/.acrawl/credentials.json`. Override the config directory with `ACRAWL_CONFIG_HOME`.

### Run

```bash
# Interactive REPL
./target/release/acrawl

# One-shot mode
./target/release/acrawl prompt "scrape all book titles and prices from books.toscrape.com"

# Resume a saved session
./target/release/acrawl --resume session.json /status /compact
```

## Examples

**Scrape a product catalog:**

```
acrawl > scrape all book titles, prices, and ratings from books.toscrape.com
```

The agent navigates to the site, reads the page, extracts the data, paginates through all 50 pages, and returns structured JSON.

**Fill and submit a form:**

```
acrawl > go to example.com/contact, fill in name "Jane Doe", email "jane@example.com",
         message "Hello", and submit the form
```

The agent locates form fields, fills them in, clicks submit, and confirms the result.

**Monitor a price:**

```
acrawl > check the current price of "Rust in Action" on books.toscrape.com
```

Single-page extraction — the agent fetches, reads, and returns the price without unnecessary navigation.

**Extract from JS-rendered pages:**

```
acrawl > get all repository names and star counts from github.com/trending
```

Static HTTP won't work here. acrawl detects React/Next.js markers and automatically escalates to a headless browser to render the JavaScript.

**Parallel multi-page crawl:**

```
acrawl > scrape the title, author, and price of every book across all 50 pages on books.toscrape.com.
         Fork a sub-agent for each page to speed this up.
```

The agent spawns up to 5 concurrent sub-agents, each on its own browser tab, to crawl pages in parallel. Results are merged when all sub-agents finish.

## Features

### 19-Tool Toolbox

#### Navigation

| Tool | Description |
|------|-------------|
| `navigate` | Go to a URL. Uses HTTP first, auto-escalates to browser when JS is detected. |
| `go_back` | Browser back button. |
| `scroll` | Scroll up or down by pixel amount (default: 500px). |
| `switch_tab` | Switch to a different browser tab by index. |
| `wait` | Wait for a CSS selector to appear or a timeout (up to 300s). |

#### Interaction

| Tool | Description |
|------|-------------|
| `click` | Click an element by CSS selector. |
| `fill_form` | Fill form fields by selector or name, with optional auto-submit. |
| `select_option` | Select a dropdown option by value, label, or index. |
| `hover` | Hover over an element to reveal tooltips or menus. |
| `press_key` | Press a keyboard key (Enter, Escape, Tab, etc.), optionally targeting an element. |
| `execute_js` | Run arbitrary JavaScript in the page context and return the result. |

#### Content Extraction

| Tool | Description |
|------|-------------|
| `page_map` | Get the page's heading hierarchy with section sizes and text previews. |
| `read_content` | Extract text by heading name or CSS selector, with offset/limit pagination for large pages. |
| `list_resources` | List all links, images, and forms on the current page. |
| `screenshot` | Capture a full-page screenshot (base64 PNG). |
| `save_file` | Download a URL to the workspace directory (path traversal protected). |

#### Agent Control

| Tool | Description |
|------|-------------|
| `fork` | Spawn a sub-agent on a new browser tab with its own goal and step budget. |
| `wait_for_subagents` | Wait for specific or all sub-agents to finish and collect results. |
| `done` | Signal task completion. Auto-waits for any active sub-agents and merges their data. |

### Sub-Agent Parallelism

The agent can fork child agents to crawl multiple pages concurrently. Each child gets its own browser tab, step budget, and independent state.

| Setting | Default | Description |
|---------|---------|-------------|
| `max_concurrent_per_parent` | 5 | Max children running in parallel per parent |
| `max_fork_depth` | 3 | Max nesting depth (agents forking agents) |
| `max_total_agents` | 10 | Global cap across all parents |
| `fork_child_max_steps` | 15 | Step budget per child agent |
| `fork_wait_timeout_secs` | 60 | Timeout waiting for sub-agents |

### Smart Fetch Routing

Every `navigate` call goes through a two-tier fetch router:

1. **HTTP first** — fast reqwest-based fetch (30s timeout, follows up to 10 redirects).
2. **Auto-escalation** — if any of the following are detected, the request is transparently replayed in a headless browser:
   - HTTP 403, 429, or 503 responses
   - JS framework markers: `__next_data__`, `__nuxt`, `__vue`, `ng-app`, `_react`, `data-reactroot`
   - Auth redirects: URLs containing `/login`, `/signin`, `/auth`, `/oauth`, `accounts.google.com`
   - Short response body (< 500 chars) with a `<noscript>` tag

When `--no-headless` / `--headed` is set, all fetches go directly through the browser.

### 24 LLM Providers

<table>
<tr><th>Category</th><th>Provider</th><th>Auth</th><th>Env Var</th></tr>
<tr><td rowspan="3"><strong>Popular</strong></td>
  <td>Anthropic</td><td>API key</td><td><code>ANTHROPIC_API_KEY</code></td></tr>
<tr><td>OpenAI</td><td>API key</td><td><code>OPENAI_API_KEY</code></td></tr>
<tr><td>Google Gemini</td><td>API key</td><td><code>GEMINI_API_KEY</code></td></tr>
<tr><td rowspan="6"><strong>Enterprise</strong></td>
  <td>Amazon Bedrock</td><td>AWS SigV4</td><td><code>AWS_ACCESS_KEY_ID</code></td></tr>
<tr><td>Azure OpenAI</td><td>Azure API key</td><td><code>AZURE_OPENAI_API_KEY</code></td></tr>
<tr><td>Google Vertex AI</td><td>GCP service account</td><td><code>GOOGLE_APPLICATION_CREDENTIALS</code></td></tr>
<tr><td>GitHub Copilot</td><td>Device OAuth</td><td>—</td></tr>
<tr><td>SAP AI Core</td><td>API key</td><td><code>SAP_AI_CORE_API_KEY</code></td></tr>
<tr><td>GitLab Duo</td><td>GitLab token</td><td><code>GITLAB_TOKEN</code></td></tr>
<tr><td rowspan="5"><strong>OSS Hosting</strong></td>
  <td>Groq</td><td>API key</td><td><code>GROQ_API_KEY</code></td></tr>
<tr><td>Cerebras</td><td>API key</td><td><code>CEREBRAS_API_KEY</code></td></tr>
<tr><td>DeepInfra</td><td>API key</td><td><code>DEEPINFRA_API_KEY</code></td></tr>
<tr><td>Together AI</td><td>API key</td><td><code>TOGETHER_API_KEY</code></td></tr>
<tr><td>Mistral AI</td><td>API key</td><td><code>MISTRAL_API_KEY</code></td></tr>
<tr><td rowspan="4"><strong>Specialized</strong></td>
  <td>Perplexity</td><td>API key</td><td><code>PERPLEXITY_API_KEY</code></td></tr>
<tr><td>xAI (Grok)</td><td>API key</td><td><code>XAI_API_KEY</code></td></tr>
<tr><td>Cohere</td><td>API key</td><td><code>COHERE_API_KEY</code></td></tr>
<tr><td>Alibaba (DashScope)</td><td>API key</td><td><code>DASHSCOPE_API_KEY</code></td></tr>
<tr><td rowspan="4"><strong>Gateways</strong></td>
  <td>OpenRouter</td><td>API key</td><td><code>OPENROUTER_API_KEY</code></td></tr>
<tr><td>Vercel AI</td><td>API key</td><td><code>VERCEL_API_KEY</code></td></tr>
<tr><td>Cloudflare Workers AI</td><td>API token</td><td><code>CLOUDFLARE_API_TOKEN</code></td></tr>
<tr><td>Cloudflare AI Gateway</td><td>API token</td><td><code>CLOUDFLARE_API_TOKEN</code></td></tr>
<tr><td rowspan="2"><strong>Other</strong></td>
  <td>Venice AI</td><td>API key</td><td><code>VENICE_API_KEY</code></td></tr>
<tr><td>Custom (OpenAI-compatible)</td><td>API key (optional)</td><td>—</td></tr>
</table>

Model aliases for quick switching: `sonnet`, `opus`, `haiku`, `4o`, `o3`, etc. Provider-prefixed names also work: `anthropic/claude-sonnet-4-6`, `openai/gpt-4o`.

### Interactive TUI

The default interface is a full terminal UI with:

- **Markdown rendering** with syntax highlighting and streaming output
- **Slash command overlay** — type `/` to see all commands with Tab completion
- **Model picker** — `/model` opens a searchable list grouped by provider category
- **Auth modal** — `/auth` walks through provider setup interactively
- **Session header** — shows current model, session ID, cost, and context usage in real time
- **Debug mode** — `/debug` toggles raw tool call input/output in the transcript
- **Reasoning effort** — `Ctrl+T` cycles through high/medium/low for reasoning models (o3, o4-mini)

**Keybindings:**

| Key | Action |
|-----|--------|
| `Enter` | Submit prompt |
| `Shift+Enter` / `Ctrl+J` | Insert newline |
| `PageUp` / `PageDown` | Scroll transcript |
| `Ctrl+T` | Cycle reasoning effort |
| `Ctrl+C` | Interrupt task (busy) or exit (idle) |
| `Esc` `Esc` | Interrupt task (double-tap while busy) |
| `Tab` | Auto-complete slash command |

A classic line-mode REPL is also available via `classic_repl: true` in settings or for `--resume` sessions.

### Session Management

- **Auto-save** — sessions are saved automatically on exit.
- **Resume** — `--resume session.json` reloads a conversation. Resume-safe slash commands (`/status`, `/compact`, `/cost`, `/config`, `/version`, `/export`, `/help`, `/clear`) can be appended to the command line.
- **Export** — `/export [file]` writes a human-readable markdown transcript.
- **Auto-compaction** — when context exceeds the token threshold (default 200K), acrawl summarizes older messages while preserving the most recent turns, unique tools used, pending work items, and referenced files.
- **Multiple sessions** — `/session list` to browse, `/session switch <id>` to switch.

### Permission Model

| Mode | Allowed Tools |
|------|---------------|
| `read-only` | `navigate` `click` `fill_form` `scroll` `screenshot` `wait` `select_option` `go_back` `execute_js` `hover` `press_key` `switch_tab` `list_resources` `page_map` `read_content` |
| `workspace-write` | Above + `save_file` |
| `danger-full-access` | Above + `fork` `wait_for_subagents` `done` |

### MCP Extensibility

acrawl supports [Model Context Protocol](https://modelcontextprotocol.io) servers, allowing you to extend the agent with custom tools. MCP tools are namespaced as `server_name__tool_name` and available alongside the built-in 19.

Supported transports: **stdio**, **SSE**, **HTTP**, **WebSocket**.

## Usage

```
acrawl [OPTIONS] [COMMAND]

Commands:
  prompt <text>      Run a single goal non-interactively
  auth [provider]    Configure provider credentials
  init               Initialize project config
  system-prompt      Print the system prompt (for debugging)

Options:
  --model MODEL            Model name or alias (sonnet, opus, 4o, o3, ...)
  --output-format FORMAT   text | json
  --permission-mode MODE   read-only | workspace-write | danger-full-access
  --resume FILE            Resume a saved session (with optional /commands)
  --compact                Compact history on resume
  --headless[=BOOL]        Force browser headless on/off
  --no-headless, --headed  Launch browser in visible mode
  --allowedTools TOOLS     Restrict available tools (comma-separated, repeatable)
  -p TEXT                  Shorthand for prompt mode
  -V, --version            Print version
```

### Slash Commands

| Command | Description | Resume-safe |
|---------|-------------|:-----------:|
| `/help` | List available commands | Yes |
| `/status` | Session info — model, tokens, cost | Yes |
| `/model [name]` | Show or switch the active model | No |
| `/compact` | Compact conversation history | Yes |
| `/clear` | Start a fresh session | Yes |
| `/cost` | Detailed cost breakdown | Yes |
| `/session [list\|switch]` | List or switch sessions | No |
| `/export [file]` | Export conversation to markdown | Yes |
| `/resume <path>` | Load a saved session | No |
| `/config [section]` | View acrawl config | Yes |
| `/auth [provider]` | Configure credentials | No |
| `/headed` | Switch to visible browser | No |
| `/headless` | Switch to headless browser | No |
| `/debug` | Toggle raw tool output | No |
| `/version` | Version and build info | Yes |
| `/exit` | Exit and save session | No |

## Configuration

All config lives in `~/.acrawl/` (override with `ACRAWL_CONFIG_HOME`).

### `credentials.json`

Managed via `acrawl auth`. Stores per-provider:

| Field | Description |
|-------|-------------|
| `active_provider` | Currently selected provider |
| `auth_method` | `api_key`, `oauth`, or `aws_sigv4` |
| `api_key` | Provider API key |
| `oauth` | OAuth tokens — access, refresh, expiry, scopes |
| `default_model` | Default model for this provider |
| `base_url` | Custom API endpoint (e.g. local Ollama, Azure resource) |

Azure additionally requires `resource_name` and `deployment_name`. Bedrock requires `aws_access_key_id`, `aws_secret_access_key`, and `region`. Vertex requires `gcp_project_id` and `gcp_region`.

### `settings.json`

Created with defaults on first run. Edit directly or via `acrawl init`.

| Field | Default | Description |
|-------|---------|-------------|
| `headless` | `true` | Run browser without a visible window |
| `max_steps` | `50` | Max agent loop iterations per goal |
| `workspace_dir` | `"workspace"` | Where `save_file` writes output |
| `classic_repl` | `false` | Use line-mode REPL instead of TUI |
| `auto_compact_input_tokens` | `200000` | Token threshold for auto-compaction |
| `reasoning_effort` | `"high"` | For reasoning models: `high` / `medium` / `low` |
| `max_concurrent_per_parent` | `5` | Max concurrent sub-agents per parent |
| `max_fork_depth` | `3` | Max nesting depth for forked agents |
| `max_total_agents` | `10` | Global cap on total agents |
| `fork_child_max_steps` | `15` | Step budget for each child agent |
| `fork_wait_timeout_secs` | `60` | Timeout for `wait_for_subagents` |

### Environment Variables

| Variable | Description |
|----------|-------------|
| `ACRAWL_CONFIG_HOME` | Override config directory (default: `~/.acrawl/`) |
| `ACRAWL_REMOTE` | Enable remote session mode |
| `ACRAWL_REMOTE_SESSION_ID` | Remote session identifier |

Provider-specific env vars (see [provider table](#24-llm-providers) above) are read as fallbacks when no `credentials.json` entry exists.

## How It Works

```mermaid
flowchart LR
    Goal([Goal\nnatural language]) --> Plan
    Plan --> Navigate --> Observe --> Act --> Extract
    Extract -->|repeat until done| Plan
    Extract --> Output([Output\nJSON / CSV])
```

1. The agent receives a goal and builds a multi-step plan via a [7-section system prompt](crates/crawler/src/prompt.rs) covering identity, operating procedure, data integrity, constraints, error recovery, completion protocol, and parallel exploration guidance.
2. Each turn, it picks from its 19 tools based on what it observes on the page.
3. `navigate` hits the FetchRouter, which tries HTTP first and auto-escalates to a headless Chromium browser when JavaScript, auth redirects, or framework markers are detected.
4. The browser is driven by an embedded Node.js subprocess (the PlaywrightBridge) speaking newline-delimited JSON over stdio — not a Rust Playwright binding.
5. For multi-page tasks, the agent can `fork` child agents onto separate browser tabs, each with independent state and step budgets. `wait_for_subagents` or `done` merges results.
6. When context grows large, auto-compaction summarizes older messages while preserving recent turns, tool usage, pending work, and file references.
7. The agent calls `done` when the goal is met, or stops when the step limit is reached.

## Architecture

```
crates/
  acrawl-cli/   CLI binary, TUI REPL, arg parsing, session management
  api/          24 provider clients (Anthropic, OpenAI, Gemini, Bedrock, Azure, ...), SSE streaming
  commands/     16 slash commands with resume-safety annotations
  crawler/      19 tools, agent loop, FetchRouter, PlaywrightBridge, sub-agent fork/join
  runtime/      ConversationRuntime, permissions, config, sessions, MCP server manager, OAuth PKCE
```

5 crates, ~37K lines of Rust, 690 tests.

## Development

```bash
cargo build --release                                     # build
cargo test --workspace                                    # run all tests
cargo clippy --workspace --all-targets -- -D warnings     # lint (pedantic)
cargo fmt --check                                         # format check
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for the full development guide.

## Changelog

See [CHANGELOG.md](CHANGELOG.md).

## Security

See [SECURITY.md](SECURITY.md) for the security policy and how to report vulnerabilities.

## License

[MIT](LICENSE)
