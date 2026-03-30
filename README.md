# Agentic Crawler

An autonomous, LLM-powered web crawler. Give it a goal in plain English and it will plan a multi-step workflow — navigating pages, filling forms, clicking buttons, and extracting structured data.

## Features

- **Full autonomy** — the agent plans, navigates, interacts, and extracts without manual step definitions
- **Dual fetching** — fast HTTP (httpx) for static pages, headless browser (Playwright) for JS-rendered and interactive sites, with automatic escalation
- **Multi-provider LLM** — supports Claude, OpenAI, and OpenAI Codex out of the box; swap with a single flag
- **OAuth login** — authenticate with your ChatGPT subscription to use Codex models without an API key
- **Structured output** — results in JSON or CSV
- **7 agent tools** — `navigate`, `click`, `fill_form`, `scroll`, `extract_data`, `screenshot`, `wait`

## Quickstart

```bash
# Clone and install
git clone <repo-url> && cd AgenticCrawler
pip install -e .

# Install the browser binary (one-time)
playwright install chromium

# Set your API key
cp .env.example .env
# Edit .env and add your ANTHROPIC_API_KEY or OPENAI_API_KEY

# Or authenticate via OAuth to use Codex models (no API key needed)
agentic-crawler login

# Run
agentic-crawler run "scrape all book titles and prices from books.toscrape.com"
```

## Usage

```
agentic-crawler <command> [OPTIONS] [ARGS]
```

### Commands

| Command | Description |
|---------|-------------|
| `run`   | Run the crawler with a natural language goal |
| `login` | Authenticate with OpenAI via OAuth (for Codex models) |

### `agentic-crawler run`

```
agentic-crawler run [OPTIONS] GOAL
```

| Option | Description |
|--------|-------------|
| `GOAL` | What you want the crawler to do, in natural language (required) |
| `-p, --provider` | LLM provider: `claude` (default), `openai`, or `codex` |
| `-m, --model` | Model name override |
| `--max-steps` | Maximum agent loop iterations (default: 50) |
| `-o, --output` | Output file path |
| `-f, --format` | Output format: `json`, `csv`, `stdout` |
| `--no-headless` | Show the browser window |
| `-v, --verbose` | Verbose logging |

### Examples

```bash
# Extract product data to a file
agentic-crawler run "find all products on example-shop.com and extract name, price, and rating" \
  -o products.json

# Use OpenAI instead of Claude
agentic-crawler run "summarize the top 5 Hacker News stories" -p openai

# Use Codex (requires `agentic-crawler login` first)
agentic-crawler run "summarize the top 5 Hacker News stories" -p codex

# Watch the browser in action
agentic-crawler run "log into example.com with user demo/demo and download my profile info" \
  --no-headless

# Output as CSV
agentic-crawler run "get the schedule from example.com/events" -f csv -o events.csv
```

## Configuration

Settings are loaded from environment variables or a `.env` file:

| Variable | Default | Description |
|----------|---------|-------------|
| `LLM_PROVIDER` | `claude` | `claude`, `openai`, or `codex` |
| `ANTHROPIC_API_KEY` | — | Required for Claude |
| `OPENAI_API_KEY` | — | Required for OpenAI (when using API key auth) |
| `OPENAI_AUTH_METHOD` | `api_key` | `api_key` or `oauth` (for OpenAI provider) |
| `CLAUDE_MODEL` | `claude-sonnet-4-20250514` | Claude model ID |
| `OPENAI_MODEL` | `gpt-4o` | OpenAI model ID |
| `CODEX_MODEL` | `codex-mini-latest` | OpenAI Codex model ID |
| `MAX_STEPS` | `50` | Max agent iterations |
| `HEADLESS` | `true` | Run browser headless |

## How it works

```
Goal (natural language)
  │
  ▼
┌─────────┐
│  PLAN   │  LLM produces a step-by-step plan
└────┬────┘
     ▼
┌─────────────────────────────────┐
│          AGENT LOOP             │
│                                 │
│  Build prompt (state + history) │
│         ▼                       │
│  LLM decides next action        │
│         ▼                       │
│  Execute action (fetch/click/…) │
│         ▼                       │
│  Observe result, update state   │
│         ▼                       │
│  Repeat until done or max steps │
└────────────┬────────────────────┘
             ▼
        Output (JSON/CSV)
```

The agent maintains a sliding context window of recent actions and observations, plus a summary of the current page (title, text, links, forms, tables). It chooses from 7 tools each turn, and automatically escalates from HTTP to a headless browser when JavaScript or interaction is needed.

## Project structure

```
src/agentic_crawler/
├── cli.py                 CLI entry point
├── config.py              Settings (pydantic-settings)
├── agent/
│   ├── loop.py            Observe-Think-Act cycle
│   ├── state.py           Agent state tracking
│   ├── prompt_builder.py  LLM prompt construction
│   └── tools.py           Tool schemas + action registry
├── llm/
│   ├── base.py            Provider protocol
│   ├── claude.py          Anthropic wrapper
│   ├── openai.py          OpenAI wrapper (API key + OAuth)
│   ├── oauth.py           OAuth PKCE flow for Codex auth
│   └── registry.py        Provider factory
├── fetcher/
│   ├── http_fetcher.py    httpx async client
│   ├── browser_fetcher.py Playwright wrapper
│   └── router.py          Auto HTTP→browser escalation
├── parser/
│   ├── html_parser.py     HTML → structured content
│   ├── readability.py     Main content extraction
│   └── structured.py      Output validation
├── actions/               One module per tool (navigate, click, …)
├── output/                JSON/CSV writer
└── utils/                 Logging, retry helpers
```

## Development

```bash
pip install -e ".[dev]"
pytest tests/ -v
ruff check src/ tests/
```

## License

MIT
