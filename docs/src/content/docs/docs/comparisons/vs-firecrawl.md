---
title: "acrawl vs Firecrawl"
description: "acrawl vs Firecrawl: a self-hosted browser agent compared with a hosted scraping API, on control, cost, observation and license."
---

*Last reviewed October 2026.*

**Short answer:** Firecrawl is primarily a hosted API for searching, scraping and interacting with websites, returning LLM-ready markdown and JSON at scale; its engine is open source (AGPL-3.0) and can be self-hosted. acrawl is a local, MIT-licensed agent you run yourself: you bring your own LLM key, it drives a browser on your machine or your real logged-in browser, and it adds DevTools observation and deterministic scripts.

## At a glance

| | acrawl | Firecrawl |
|---|---|---|
| Where it runs | Your machine, server or CI | Firecrawl's cloud, or self-hosted |
| Pricing model | Free; you pay your own LLM provider | Hosted plans; self-hosting is free |
| Output | Structured JSON from a goal, or raw page content via tools | Markdown, JSON, screenshots, crawl results |
| Interaction | Click, fill, select, scroll, keys, device emulation | Actions and AI-driven Interact on scraped pages |
| Autonomous agent | Local agent with sub-agents, 25 LLM providers | Hosted `/agent` endpoint |
| Your logged-in browser | Yes, through the acrawl Bridge extension | Not its model; it uses its own browsers |
| DevTools observation | Network, console, WebSocket, performance, cookies, storage, coverage, axe-core | Not a focus |
| Repeatable runs without LLM calls | Deterministic JSON scripts | N/A |
| MCP server | Built in | Yes |
| License | MIT | AGPL-3.0 engine |

## Choose acrawl if

- You want everything to run locally with your own LLM key and no per-page fees.
- You need to work inside sites you're logged into.
- You want to inspect how a page behaves, not just what it contains.

## Choose Firecrawl if

- You want a managed API that handles crawling infrastructure and scale for you.
- You mostly need clean, LLM-ready content from many URLs.
- You'd rather not run browsers yourself.

## The same job, two ways

Take "collect the name and price of every product in a 40-page catalog, every morning".

**With Firecrawl**, you call its crawl or extract endpoint with the start URL and a schema, and it returns clean records from its own infrastructure. You pay per page on a hosted plan, or run the AGPL engine yourself.

**With acrawl**, you describe the job once:

```text
acrawl > get the name and price of every product on example.com/catalog, following
         pagination, and save the crawl as a script called catalog
```

The agent works out the pagination and the selectors, returns the data, and saves the steps as a [deterministic script](/AgenticCrawler/docs/guides/deterministic-scripts/). Every morning after that, a cron job runs it with no model at all:

```bash
acrawl run catalog --format jsonl --flatten --output catalog.jsonl
```

The first run costs some model tokens; the daily runs cost nothing but the machine they run on.

## Using them together

They don't compete for every job. A common split:

- Firecrawl for broad, shallow coverage: turning thousands of public URLs into Markdown for search or retrieval.
- acrawl for deep, interactive work: logging in with [your own browser](/AgenticCrawler/docs/guides/real-browser/), working through multi-step flows, and [debugging](/AgenticCrawler/docs/guides/devtools-debugging/) the pages your own team ships.

Both ship MCP servers, so a coding agent such as Claude Code can have both connected and pick the right one per task.

## Common questions

**Is acrawl a self-hosted Firecrawl?** No. Firecrawl is a scraping service with an API; acrawl is an agent with a browser. acrawl has no HTTP API or job queue to host, and Firecrawl has no local agent that works through a task in your own browser.

**Which is cheaper?** For a few repeated jobs, acrawl, because scripts replay without model calls and there are no per-page fees. At very large scale, compare Firecrawl's plan prices with your own LLM and server costs.

**Does acrawl return Markdown like Firecrawl?** Yes. `navigate` returns a page as `markdown`, `fit_markdown` (main content only), `text` or `html`, and the agent can shape results into whatever JSON you ask for.
