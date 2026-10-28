---
title: "How acrawl works"
description: "Inside acrawl: the agent loop, HTTP-first fetching with browser escalation, sub-agents with URL claiming, the browser backends, and known limitations."
head:
  - tag: title
    content: "How acrawl works: agent loop, fetch routing and sub-agents"
  - tag: meta
    attrs:
      property: og:title
      content: "How acrawl works: agent loop, fetch routing and sub-agents"
---

**Short answer:** acrawl runs a loop: the model reads the goal and the current page, calls one of 43 tools, sees the result, and repeats until the goal is met or the step limit is reached. Pages are fetched over plain HTTP first and moved into a real browser only when they need JavaScript. Large jobs can be split across sub-agents, each in its own tab.

![acrawl architecture: the TUI, one-shot prompt and MCP entry points feed a runtime turn loop that streams from an LLM provider and runs tools, which fetch over HTTP, escalate to CloakBrowser, or drive the user's Chrome through an extension bridge.](/AgenticCrawler/media/architecture.svg)

## The agent loop

1. **Goal in.** You describe what you want in plain English, from the terminal UI, `acrawl prompt` or the MCP server's `run_goal`.
2. **Plan.** The system prompt gives the model an operating procedure: how to explore a page, keep data accurate, recover from errors, and decide when it is finished.
3. **Act.** Each turn the model streams a reply and calls tools: `navigate`, `page_map`, `click`, `fill_form`, `read_content` and so on. acrawl runs them and returns the results.
4. **Observe.** Tools return compact, model-friendly views of the page. `page_map` lists interactive elements with short references such as `@e12`, so the model clicks by reference instead of writing fragile CSS selectors.
5. **Repeat or stop.** The loop ends when the model answers without calling a tool, all sub-agents have finished, or `max_steps` (default 50) is reached.

When the conversation grows past `auto_compact_input_tokens` (default 200,000), acrawl [compacts it](/AgenticCrawler/docs/configuration/#compaction): recent turns stay word for word and older ones become a summary. The browser keeps its state.

## Fetching: HTTP first, browser when needed

Every `navigate` call goes through a router:

1. **HTTP first.** A plain request with a 30-second timeout and up to 10 redirects. Static pages are done in milliseconds and cost no browser.
2. **Escalate when the page needs it.** The request is replayed in a headless browser when the response is a 403, 429 or 503, carries framework markers such as `__next_data__`, `__nuxt` or `ng-app`, redirects to a login page, or looks like an empty single-page-app shell. Pages that already embed their data, such as Next.js pages with `__NEXT_DATA__`, stay on HTTP.

With `--headed`, everything goes straight to the browser. [Scrape JavaScript-rendered sites](/AgenticCrawler/docs/guides/scrape-js-sites/) covers this in detail.

## Browser backends

| Backend | When it is used | What it is |
|---|---|---|
| **CloakBrowser** (default) | Every escalated fetch and every interactive tool | A stealth build of Chromium, driven by a small Node.js bridge that acrawl starts as a subprocess. Downloaded on first use. |
| **Your own browser** | After `/extension`, or with `browser_backend` set to `extension` | The acrawl Bridge extension, driving Chrome, Edge or Brave through the DevTools Protocol over a local WebSocket. See [Use your real browser](/AgenticCrawler/docs/guides/real-browser/). |

In extension mode acrawl never quietly falls back to CloakBrowser. If the extension isn't connected, the tool returns an error, because the point of extension mode is your signed-in session.

## Sub-agents and URL claiming

For multi-page jobs the agent can `fork` child agents. Each child gets its own tab, its own step budget (`fork_child_max_steps`, default 100) and its own state, and the parent collects results with `wait_for_subagents`.

| Limit | Default |
|---|---|
| Children running at once, per parent | 5 |
| Nesting depth | 3 |
| Sub-agents in the whole run | 10 |

Before a child starts, the URLs it will cover are claimed in a shared registry, so two siblings never crawl the same page. A claim can be one URL, a list of URLs, or a regular expression. The first claim wins; a conflicting fork fails immediately with the owner's name, and the parent picks a different split. Claims are released when the child finishes or is cancelled.

## Deterministic scripts

Not every step needs a model. [Scripts](/AgenticCrawler/docs/guides/deterministic-scripts/) are JSON programs with loops, conditions and parallel branches, built from the same tools. They run with zero model calls, so a crawl the agent worked out once can be replayed for free with `acrawl run`.

## Inside the binary

acrawl is a Rust workspace of 11 crates that compile into one executable:

| Crate | Role |
|---|---|
| `core` | Shared types and traits |
| `api` | Clients for the 25 LLM providers, with streaming |
| `browser` | HTTP router, CloakBrowser bridge, extension bridge |
| `agent` | The 43 tools, the agent loop, sub-agents |
| `runtime` | Conversation loop, sessions, compaction, MCP client |
| `render` | Markdown and tool-output rendering |
| `mcp-server` | The built-in MCP server and editor installer |
| `tui`, `ui`, `cli`, `commands` | Terminal UI, shared app layer, entry point, slash commands |

## Known limitations

| Situation | What happens |
|---|---|
| **CAPTCHAs and bot challenges** | CloakBrowser avoids most bot detection, but a puzzle CAPTCHA or proof-of-work challenge will stop it. Use [your own browser](/AgenticCrawler/docs/guides/real-browser/), where you already have a trusted session. When an invisible reCAPTCHA v3 silently rejects a form, acrawl reports `CaptchaDetected` instead of a false success. |
| **SMS or authenticator codes** | The agent can type a code you paste in, but can't receive or generate one. |
| **Content behind a login** | Use extension mode, so the agent works inside your signed-in browser. |
| **Endless infinite scroll** | The agent can scroll to load more, but a feed with no end needs a step limit or a stopping rule in the goal. |
| **PDFs and other binary files** | `save_file` downloads them, but the agent can't read their text. |
| **Hardware fingerprinting** | Checks that fingerprint the GPU through WebGL can't be fully spoofed. |
| **Mouse-movement analysis** | Sites that score real mouse paths may flag a headless browser. Use extension mode. |
