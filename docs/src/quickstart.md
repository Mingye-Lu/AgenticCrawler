# Quick start

## 1. Connect an LLM provider

```bash
acrawl auth anthropic   # or: openai, other
```

Credentials are stored in `~/.acrawl/credentials.json`. Set `ACRAWL_CONFIG_HOME` to use a different config directory.

## 2. Run a crawl

Start the interactive terminal UI:

```bash
acrawl
```

Or run a single goal and exit, which suits scripts and CI:

```bash
acrawl prompt "scrape all book titles and prices from books.toscrape.com"
```

Resume a saved session:

```bash
acrawl --resume session.json /status /compact
```

## Example goals

**Scrape a product catalog**

```text
acrawl > scrape all book titles, prices, and ratings from books.toscrape.com
```

The agent reads the page, extracts the data, follows pagination through all 50 pages and returns JSON.

**Fill and submit a form**

```text
acrawl > go to example.com/contact, fill in name "Jane Doe", email "jane@example.com",
         message "Hello", and submit the form
```

**Extract from a JavaScript-rendered page**

```text
acrawl > get all repository names and star counts from github.com/trending
```

Plain HTTP won't see this content. acrawl detects the framework markers and escalates to a headless browser on its own.

**Crawl in parallel**

```text
acrawl > scrape the title, author, and price of every book across all 50 pages on
         books.toscrape.com. Fork a sub-agent for each page to speed this up.
```

The agent runs up to 5 sub-agents at once, each in its own tab, and merges their results.

## Next

- [Use acrawl from Claude Code, Cursor or VS Code](mcp-server.md)
- [Turn a crawl into a repeatable script](guides/deterministic-scripts.md)
