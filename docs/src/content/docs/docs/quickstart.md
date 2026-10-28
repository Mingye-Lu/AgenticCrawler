---
title: "Quick start"
description: "Connect an LLM provider and run your first acrawl crawl in a few minutes, from the terminal UI or a one-shot prompt."
head:
  - tag: title
    content: "acrawl quick start: run your first AI web crawl"
  - tag: meta
    attrs:
      property: og:title
      content: "acrawl quick start: run your first AI web crawl"
---

This page takes you from a fresh [install](/AgenticCrawler/docs/install/) to a finished crawl. You need an API key for one LLM provider, or a local model server such as Ollama.

## 1. Connect an LLM provider

```bash
acrawl auth anthropic   # or: openai, google, other, ... (acrawl auth list shows all 25)
```

acrawl asks for your key and stores it in `~/.acrawl/credentials.json`. Set `ACRAWL_CONFIG_HOME` to use a different config directory. For a local model, run `acrawl auth other --base-url http://localhost:11434/v1`. [LLM providers](/AgenticCrawler/docs/providers/) covers every provider and how to pick a model.

Check it worked:

```bash
acrawl auth status
```

## 2. Run a crawl

Start the interactive terminal UI:

```bash
acrawl
```

Or run a single goal and exit, which suits scripts and CI:

```bash
acrawl prompt "scrape all book titles and prices from books.toscrape.com"
```

The terminal UI streams the agent's reasoning and every tool call as it happens. Type a goal and press Enter; press `Esc` twice to interrupt. Sessions save automatically when you quit with `/exit`.

## 3. Watch it, steer it, check the cost

- **See the browser.** Start with `acrawl --headed`, or type `/headed`, to watch the agent click through the page in a visible window.
- **Steer it.** If the agent heads the wrong way, interrupt and add a message such as "only the first page, please". It keeps the context it has built so far.
- **Check the cost.** `/cost` shows tokens and spend for the session; `/status` shows the model and context use.
- **Switch model.** `/model` opens a searchable list of every configured provider's models.

## 4. Get the data out

In the terminal UI the agent replies with the data, normally as JSON or a table. For scripts and pipelines, ask for JSON output:

```bash
acrawl --output-format json prompt "list the 10 newest posts on example.com/blog with title and date" > result.json
```

The JSON holds the agent's final `message`, the `model`, the `tool_uses` it made and token `usage`. Files the agent downloads with `save_file` go to the `output` folder, which you can change with `acrawl config set output_dir <path>`.

## Write goals that work

| Instead of | Try |
|---|---|
| "get the products" | "get the name, price and rating of every product on example.com/shop, following pagination" |
| "check the site" | "open example.com, list any console errors and any network requests that failed" |
| "log in and get my orders" | Use [your own browser](/AgenticCrawler/docs/guides/real-browser/), where you're already signed in, then "list my last 5 orders" |

Name the site, the fields you want and when to stop. The agent does better with a clear finish line than with an open-ended "everything".

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

- [Use acrawl from Claude Code, Cursor or VS Code](/AgenticCrawler/docs/mcp-server/)
- [Turn a crawl into a repeatable script](/AgenticCrawler/docs/guides/deterministic-scripts/) that runs without a model
- [Every command and slash command](/AgenticCrawler/docs/cli/)
