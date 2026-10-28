---
title: "FAQ"
description: "Answers about acrawl: what it is, supported LLMs, MCP clients, JavaScript sites, logins and CAPTCHAs, costs, and how it compares."
---

## What is acrawl?

acrawl (the project is called AgenticCrawler) is an open-source, MIT-licensed browser agent and MCP server written in Rust. You describe a web task in plain English and it navigates, interacts and returns structured data. It also exposes DevTools-style observation (network, console, performance, cookies, storage, coverage, accessibility) as agent tools.

## Do I need to write code?

No. Goals are plain English, for example `acrawl prompt "scrape all book titles and prices from books.toscrape.com"`. If you want repeatable, cheaper runs, you can optionally save the repetitive part as a [JSON script](/AgenticCrawler/docs/guides/deterministic-scripts/).

## Is it really a single binary?

The `acrawl` executable is one compiled Rust binary, with no Python environment or Docker needed. Browser automation drives a bundled stealth browser (CloakBrowser) through a small Node.js bridge, so browser features need Node.js 20+. The install script sets this up.

## Which LLMs does it work with?

25 providers, including Anthropic, OpenAI, Google Gemini, DeepSeek, AWS Bedrock, Azure OpenAI, Vertex AI, GitHub Copilot, Groq, Mistral, xAI, Cohere, Alibaba DashScope and OpenRouter, plus any OpenAI-compatible endpoint, including local models.

## Can I use it from Claude Code, Cursor or VS Code?

Yes. Run `acrawl mcp install` and pick your clients. The MCP server exposes 40 tools; see [Use acrawl as an MCP server](/AgenticCrawler/docs/mcp-server/).

## Do the MCP tools need an API key?

Only `run_goal` does, because it runs acrawl's own agent loop. The 32 browser and DevTools tools and the 7 script tools work without any LLM key.

## Can it handle JavaScript-heavy sites and SPAs?

Yes. acrawl fetches over HTTP first and escalates to a headless browser when it detects framework markers, auth redirects or an empty SPA shell. See [Scrape JavaScript-rendered sites](/AgenticCrawler/docs/guides/scrape-js-sites/).

## Can it log in to sites or get past CAPTCHAs?

It can fill login forms, and it can type a 2FA code you paste in, but it can't receive or generate codes. It won't solve image or proof-of-work CAPTCHAs. For logged-in or heavily protected sites, use [your real browser](/AgenticCrawler/docs/guides/real-browser/) through the acrawl Bridge extension.

## How does it keep costs down?

Static pages skip the browser entirely. `fit_markdown` strips boilerplate, typically saving 30–60% of tokens. Deterministic scripts run repetitive steps with no LLM calls. Sub-agents work in parallel with their own step budgets.

## Does it respect robots.txt and site terms?

acrawl is a tool you run, and you're responsible for how you use it. Check a site's terms and robots.txt, and keep request rates reasonable.

## How is it different from browser-use, Playwright MCP or Firecrawl?

See the [comparison overview](/AgenticCrawler/docs/comparisons/overview/).
