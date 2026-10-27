# acrawl vs Firecrawl

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
