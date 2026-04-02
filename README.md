# acrawl — AgenticCrawler

An autonomous, LLM-powered web crawler. Give it a goal in plain English and it will navigate pages, fill forms, click buttons, and extract structured data.

## Quick Start

### Build

\\\ash
# Requires Rust toolchain (https://rustup.rs)
cargo build --release
# Binary: target/release/acrawl (or acrawl.exe on Windows)
\\\

### Install Playwright (for browser automation)

\\\ash
npx playwright install chromium
\\\

### Configure

\\\ash
cp .env.example .env
# Edit .env: set ANTHROPIC_API_KEY, OPENAI_API_KEY, or use acrawl login for Codex
\\\

### Run

\\\ash
./target/release/acrawl
# › extract the page title from example.com
\\\

## Features

- **Full autonomy** — agent plans, navigates, interacts, extracts
- **Dual fetching** — HTTP for static pages, Playwright browser for JS-rendered sites
- **15 browser tools** — navigate, click, fill_form, scroll, extract_data, screenshot, wait, select_option, go_back, execute_js, hover, press_key, switch_tab, list_resources, save_file
- **Multi-provider LLM** — Anthropic Claude, OpenAI, Codex (OAuth login)
- **Structured output** — JSON, CSV, plain text
- **Session persistence** — conversation history, permission model, auto-compaction

## CLI Usage

\\\
acrawl [OPTIONS] [COMMAND]

Options:
  --model MODEL          Set model (sonnet/opus/haiku or full model name)
  --output-format FORMAT Output format: text, json (default: text)

Commands:
  login    Authenticate via OAuth (for Codex models)
  logout   Clear stored credentials
  prompt   One-shot prompt (non-interactive)
  init     Initialize project config
\\\

## Slash Commands (REPL)

| Command | Description |
|---------|-------------|
| /help   | Show available commands |
| /status | Show session status (model, tokens, cost) |
| /model  | Show or switch model |
| /compact | Compact conversation history |
| /clear  | Clear conversation |
| /cost   | Show cost breakdown |

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| ANTHROPIC_API_KEY | — | Required for Claude |
| OPENAI_API_KEY | — | Required for OpenAI |
| CODEX_MODEL | codex-mini-latest | Codex model |
| CLAUDE_MODEL | claude-sonnet-4-6 | Claude model |
| OPENAI_MODEL | gpt-4o | OpenAI model |
| MAX_STEPS | 50 | Max agent iterations |
| HEADLESS | true | Run browser headless |

## Development

\\\ash
cargo test --workspace         # Run all tests
cargo clippy --workspace --all-targets -- -D warnings  # Lint
cargo fmt                      # Format
cargo build --release          # Release build
\\\

## Architecture

\\\
crates/
├── acrawl-cli/   CLI binary: REPL, slash commands, session management
├── api/          LLM providers: Anthropic, OpenAI, Codex (OAuth)
├── commands/     Shared slash command registry
├── crawler/      Browser tools, agent loop, HTTP fetcher, Playwright bridge
└── runtime/      Session, config, permissions, conversation loop
\\\

## License

MIT
