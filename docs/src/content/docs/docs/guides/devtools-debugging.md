---
title: "Debug a live page: network, console, performance, accessibility"
description: "Give an AI agent DevTools: network requests, console errors, WebSockets, performance, cookies, coverage and axe-core accessibility audits."
---

**Short answer:** acrawl's agent and MCP server include the DevTools surface as callable tools. An AI assistant can open a page, then ask what requests it made, which console errors it threw, how fast it loaded and which WCAG rules it breaks, instead of guessing from the HTML.

## Ask in plain English

```text
acrawl > open localhost:3000/checkout, submit the form with test data, and tell me
         which network requests failed and what the console logged
```

```text
acrawl > audit example.com for accessibility problems and group them by severity
```

From an MCP client such as Claude Code, the same tools are available directly, so your coding agent can check its own front-end changes in a real browser.

## The tools

| Question | Tool |
|---|---|
| What requests did the page make, and which were slow or failed? | `list_network_activity`, then `inspect_request` on an `@rN` id |
| What did the console log? | `list_page_logs` (deduplicated, filterable by level), then `inspect_log` for stack traces |
| What went over the WebSocket? | `list_websocket_activity`, `inspect_websocket` |
| How fast did it load? | `get_page_performance`: TTFB, DOM timings, top 20 resources by size |
| What cookies and storage does it set? | `inspect_cookies` (with security checks and third-party detection), `inspect_storage` |
| How much of the JS and CSS is actually used? | `measure_coverage` |
| Does it pass WCAG checks? | `audit_accessibility` (axe-core, grouped by impact) |
| What if this request failed or returned something else? | `intercept_network` to block or mock, then `refresh` |
| How does it look on a phone? | `set_device` with presets such as `iphone_15` or `pixel_7` |

## Time-scoped questions

Interactions advance a sequence counter. Network and log tools accept a seq window, so you can ask "what happened after I clicked submit?" without wading through the initial page load.
