# acrawl

A native Rust web crawler powered by LLMs. Describe a goal in plain English — `acrawl` plans a multi-step workflow, navigates pages, fills forms, clicks buttons, and returns structured data.

Single binary. No Python runtime. Playwright for browser automation.

## Quick start

```bash
cargo build --release

npm install          # installs Playwright and downloads Chromium automatically

./target/release/acrawl auth   # configure provider credentials (API key or OAuth)
./target/release/acrawl
```

```
acrawl › scrape all book titles and prices from books.toscrape.com
```

## What it does

The agent receives a natural-language goal, builds a plan, then enters a loop:

```
Goal
 │
 ▼
Plan  →  Navigate  →  Observe  →  Act  →  Extract
 │                                           │
 └───────────── repeat until done ───────────┘
                                             │
                                          Output
                                       (JSON / CSV)
```

It chooses from 16 browser tools each turn, automatically escalates from fast HTTP fetches to a headless browser when JavaScript or interaction is needed, and stops when the goal is met or the step limit is reached.

## Features

- **Single binary** — `cargo build --release` produces one executable, no interpreter needed
- **Dual fetching** — static pages served via HTTP (reqwest), JS-rendered pages escalated to Playwright
- **16 browser tools** — `navigate`, `click`, `fill_form`, `scroll`, `screenshot`, `wait`, `select_option`, `go_back`, `execute_js`, `hover`, `press_key`, `switch_tab`, `list_resources`, `save_file`, `page_map`, `read_content`
- **3 LLM providers** — Anthropic Claude, OpenAI, Codex (with OAuth PKCE login)
- **Structured output** — JSON, CSV, or plain text
- **Interactive REPL** — markdown rendering, syntax highlighting, spinners, slash commands
- **Session persistence** — save / resume conversations, auto-compaction, permission model

## Usage

```
acrawl [OPTIONS] [COMMAND]

Commands:
  prompt <text>   One-shot (non-interactive)
  auth [provider] Configure provider credentials (anthropic, openai, other)
  login           Authenticate via OAuth for Codex
  logout          Clear stored credentials
  init            Initialize project config

Options:
  --model MODEL            Model name or alias (sonnet, opus, haiku, gpt-4o)
  --output-format FORMAT   text | json
  --permission-mode MODE   read-only | workspace-write | danger-full-access
```

### REPL slash commands

| Command    | Description                          |
|------------|--------------------------------------|
| `/help`    | Show available commands               |
| `/status`  | Session info — model, tokens, cost   |
| `/model`   | Show or switch model                 |
| `/compact` | Compact conversation history         |
| `/clear`   | Clear conversation                   |
| `/cost`    | Cost breakdown                       |
| `/session` | Resume a previous session            |
| `/export`  | Export conversation to file           |

## Configuration

All configuration is stored in `~/.acrawl/` (override with `ACRAWL_CONFIG_HOME`).

### `credentials.json` — LLM provider credentials

Managed via `acrawl auth [anthropic|openai|other]`. Stores per-provider:

| Field            | Description                                      |
|------------------|--------------------------------------------------|
| `active_provider`| Which provider is currently selected              |
| `auth_method`    | `api_key` or `oauth`                              |
| `api_key`        | API key (Anthropic, OpenAI, or custom)            |
| `oauth`          | OAuth tokens (access, refresh, expiry, scopes)    |
| `default_model`  | Default model for this provider                   |
| `base_url`       | Custom API origin (e.g. local Ollama endpoint)    |

### `settings.json` — runtime settings

Created automatically with defaults; edit directly or via `acrawl init`.

| Field                      | Default       | Description                        |
|----------------------------|---------------|------------------------------------|
| `headless`                 | `true`        | Run browser headless               |
| `max_steps`                | `50`          | Max agent loop iterations          |
| `workspace_dir`            | `"workspace"` | Directory for saved files          |
| `permission_mode`          | `"read-only"` | `read-only` / `workspace-write` / `danger-full-access` |
| `classic_repl`             | `false`       | Use classic REPL instead of TUI    |
| `auto_compact_input_tokens`| `200000`      | Auto-compact threshold in tokens   |

## Architecture

```
crates/
├── acrawl-cli/   REPL, arg parsing, session management, TUI rendering
├── api/          Anthropic + OpenAI + Codex clients, SSE streaming
├── commands/     Slash command registry
├── crawler/      19 tools, agent loop, Playwright bridge, HTTP fetcher
└── runtime/      ConversationRuntime, config, permissions, sessions
```

5 crates, ~23K lines of Rust, 316 tests.

## Development

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
cargo build --release
```

## License

MIT
