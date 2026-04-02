# acrawl

A native Rust web crawler powered by LLMs. Describe a goal in plain English — `acrawl` plans a multi-step workflow, navigates pages, fills forms, clicks buttons, and returns structured data.

Single binary. No Python runtime. Playwright for browser automation.

## Quick start

```bash
cargo build --release

npx playwright install chromium

cp .env.example .env
# set ANTHROPIC_API_KEY or OPENAI_API_KEY

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

It chooses from 15 browser tools each turn, automatically escalates from fast HTTP fetches to a headless browser when JavaScript or interaction is needed, and stops when the goal is met or the step limit is reached.

## Features

- **Single binary** — `cargo build --release` produces one executable, no interpreter needed
- **Dual fetching** — static pages served via HTTP (reqwest), JS-rendered pages escalated to Playwright
- **15 browser tools** — `navigate`, `click`, `fill_form`, `scroll`, `extract_data`, `screenshot`, `wait`, `select_option`, `go_back`, `execute_js`, `hover`, `press_key`, `switch_tab`, `list_resources`, `save_file`
- **3 LLM providers** — Anthropic Claude, OpenAI, Codex (with OAuth PKCE login)
- **Structured output** — JSON, CSV, or plain text
- **Interactive REPL** — markdown rendering, syntax highlighting, spinners, slash commands
- **Session persistence** — save / resume conversations, auto-compaction, permission model

## Usage

```
acrawl [OPTIONS] [COMMAND]

Commands:
  prompt <text>   One-shot (non-interactive)
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

All settings come from environment variables or `.env`:

| Variable           | Default              | Description                        |
|--------------------|----------------------|------------------------------------|
| `ANTHROPIC_API_KEY`| —                    | Required for Claude                |
| `OPENAI_API_KEY`   | —                    | Required for OpenAI                |
| `CLAUDE_MODEL`     | `claude-sonnet-4-6`  | Claude model                       |
| `OPENAI_MODEL`     | `gpt-4o`             | OpenAI model                       |
| `CODEX_MODEL`      | `codex-mini-latest`  | Codex model (requires `acrawl login`) |
| `MAX_STEPS`        | `50`                 | Max agent loop iterations          |
| `HEADLESS`         | `true`               | Run browser headless               |
| `WORKSPACE_DIR`    | `workspace`          | Directory for saved files          |

## Architecture

```
crates/
├── acrawl-cli/   REPL, arg parsing, session management, TUI rendering
├── api/          Anthropic + OpenAI + Codex clients, SSE streaming
├── commands/     Slash command registry
├── crawler/      15 tools, agent loop, Playwright bridge, HTTP fetcher
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
