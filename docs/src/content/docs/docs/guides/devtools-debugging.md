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

From an MCP client such as Claude Code, the same tools are available directly, so your coding agent can check its own front-end changes in a real browser. Set that up with [the MCP server guide](/AgenticCrawler/docs/mcp-server/).

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

## A worked example: why does checkout fail?

Say a checkout button on your local app does nothing. From Claude Code with acrawl's MCP server connected, you ask: *"open localhost:3000/checkout, click Pay, and find out why nothing happens."* A typical run looks like this:

1. `navigate` to `http://localhost:3000/checkout`, then `page_map` to find the Pay button, say `@e14`.
2. `click` on `@e14`. The click advances the sequence counter, say to 7.
3. `list_network_activity` with `since: 7` and `filter: "failed"`. One request stands out: `POST /api/payments` returned 422.
4. `inspect_request` on that request's id shows the response body: `{"error": "currency is required"}`, and the request payload confirms the form never sent a currency.
5. `list_page_logs` with `level: "error"` and `since: 7` shows the unhandled rejection the front end swallowed.

The agent now has the failing request, the server's reason and the client-side error, and can fix the form, rather than guessing from the page source. Because every step uses a real browser, the fix can be checked the same way.

## Test the unhappy paths

`intercept_network` changes what the page receives, so you can see how it copes when things go wrong, without touching the backend:

| To test | Call |
|---|---|
| A third-party script being blocked | `action: "block"`, `pattern: "*analytics*"` |
| An API outage | `action: "mock_response"`, `pattern: "*/api/v2/*"`, with `mock.status` 503 |
| An empty result | `mock_response` returning `body: "[]"` with `content_type: "application/json"` |
| Back to normal | `action: "clear_all"` |

Rules apply to new requests, so call `refresh` after adding one to replay the page load with it in place. Patterns are URL globs; prefix one with `re:` for a regular expression.

## Check pages the way users see them

- **Phones and tablets.** `set_device` with a preset such as `iphone_15` or `pixel_7` changes the viewport, user agent and touch support. Pair it with `screenshot` to compare layouts.
- **Slow pages.** `get_page_performance` gives time to first byte, DOM timings and the 20 largest resources, which is usually enough to spot the oversized image or blocking script.
- **Unused code.** `measure_coverage` shows how much of each JavaScript and CSS file the page actually ran, which points at what to split or lazy-load.
- **Accessibility.** `audit_accessibility` runs axe-core and groups violations by impact, with the rule that failed and the elements that break it.

## Use it in your own browser

All of these tools also work in [extension mode](/AgenticCrawler/docs/guides/real-browser/), so you can debug a page as your signed-in user, including staging sites behind single sign-on.
