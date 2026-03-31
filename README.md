# Agentic Crawler

An autonomous, LLM-powered web crawler. Give it a goal in plain English and it will plan a multi-step workflow — navigating pages, filling forms, clicking buttons, and extracting structured data.

## Features

- **Full autonomy** — the agent plans, navigates, interacts, and extracts without manual step definitions
- **Dual fetching** — fast HTTP (httpx) for static pages, headless browser (Playwright) for JS-rendered and interactive sites, with automatic escalation
- **Multi-agent forking** — spawn parallel subagents to explore multiple pages or approaches simultaneously, with automatic result aggregation
- **Multi-provider LLM** — supports Claude, OpenAI, and OpenAI Codex out of the box; swap with a single flag
- **OAuth login** — authenticate with your ChatGPT subscription to use Codex models without an API key
- **Structured output** — results in JSON or CSV
- **File downloads** — save images, PDFs, and other resources to a local workspace directory
- **15 agent tools** — `navigate`, `click`, `fill_form`, `scroll`, `extract_data`, `screenshot`, `wait`, `select_option`, `go_back`, `execute_js`, `hover`, `press_key`, `switch_tab`, `list_resources`, `save_file`, plus `fork` and `wait_for_subagents` for multi-agent workflows

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
| `-w, --workspace` | Directory for saved files (default: `workspace`) |
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

# Download images to a workspace directory
agentic-crawler run "download all product images from example-shop.com" -w ./downloads
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
| `WORKSPACE_DIR` | `workspace` | Directory for saved files |
| `MAX_CONCURRENT_PER_PARENT` | `5` | Max concurrent subagents per parent |
| `MAX_FORK_DEPTH` | `3` | Max fork recursion depth |
| `MAX_TOTAL_AGENTS` | `10` | Max total agents in fork tree |
| `FORK_CHILD_MAX_STEPS` | `15` | Max steps for forked child agents |
| `FORK_WAIT_TIMEOUT` | `60` | Seconds to wait for subagent completion |

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
│  fork ──► subagent(s) on new    │
│           browser tabs          │
│         ▼                       │
│  Repeat until done or max steps │
└────────────┬────────────────────┘
             ▼
        Output (JSON/CSV)
```

The agent maintains a sliding context window of recent actions and observations, plus a summary of the current page (title, text, links, forms, tables). It chooses from 15 tools each turn, and automatically escalates from HTTP to a headless browser when JavaScript or interaction is needed.

### Multi-agent forking

When a task benefits from parallel exploration, the agent can `fork` subagents. Each subagent gets its own browser tab (within the same browser context) and a copy of the parent's action history. Subagents work independently and their extracted data is merged back into the parent when they complete.

Fork limits are configurable to prevent runaway agents — see the configuration table above.

## Project structure

```
src/agentic_crawler/
├── cli.py                 CLI entry point
├── config.py              Settings (pydantic-settings)
├── agent/
│   ├── crawl_agent.py     CrawlAgent class (plan, loop, fork handling)
│   ├── display.py         AgentDisplay protocol + LiveDashboard (Rich TUI)
│   ├── loop.py            run_agent() entry point
│   ├── manager.py         AgentManager (fork lifecycle & limits)
│   ├── state.py           Agent state tracking + fork()
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
├── actions/               One module per tool (15 actions)
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
