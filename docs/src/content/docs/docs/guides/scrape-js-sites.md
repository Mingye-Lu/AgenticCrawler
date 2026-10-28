---
title: "Scrape JavaScript-rendered sites without code"
description: "Scrape JavaScript-rendered sites and SPAs from a plain-English goal: acrawl fetches over HTTP and escalates to a stealth browser when needed."
---

**Short answer:** give acrawl the goal in plain English. It fetches over HTTP first and switches to a headless stealth browser automatically when the page needs JavaScript, so you don't choose the method or write selectors.

```bash
acrawl prompt "get all repository names and star counts from github.com/trending"
```

## How acrawl decides when to use a browser

Every `navigate` call goes through a two-tier fetch router:

1. **HTTP first.** A fast fetch with a 30-second timeout that follows up to 10 redirects.
2. **Escalate when needed.** acrawl replays the request in a headless browser if it sees any of these:
   - HTTP 403, 429 or 503 responses
   - JavaScript framework markers such as `__next_data__`, `__nuxt`, `__vue`, `ng-app` or `data-reactroot`
   - Auth redirects (`/login`, `/signin`, `/oauth`, `accounts.google.com` and similar)
   - A very short body with a `<noscript>` tag
   - An empty SPA shell: framework asset paths with no embedded data, empty mount points, "enable JavaScript" messages

Pages that already embed their data (for example a populated `__NEXT_DATA__` blob) stay on the fast HTTP path.

Pass `--headed` to send every fetch through a visible browser instead.

## Get only the content that matters

`navigate` can return `markdown`, `fit_markdown`, `text` or `html`. `fit_markdown` is the recommended default: it strips navigation, footers, ads and other boilerplate, then scores the remaining blocks for text density and structure. On typical pages this saves 30–60% of tokens.

Since v0.16, `navigate` also accepts `focus`, a phrase or list of phrases such as `["price", "specs"]`. With goal-aware pruning turned on (`optimization.semantic_prune`, off by default), acrawl splits the page into record-level blocks, scores each against the focus and collapses unrelated ones into `[N blocks pruned]` markers. This feature sends page text to an external scoring API (TypeSafe), needs its API key, and is never used with the logged-in extension backend.

## Infinite scroll and lazy loading

The agent can `scroll` to trigger lazy loading. For feeds with no natural end, give the goal a limit ("the first 200 posts") or set a step budget.

## When it won't work

- **Unsolvable CAPTCHAs** (image puzzles, proof-of-work challenges) stop the run. Use your [real browser](/AgenticCrawler/docs/guides/real-browser/), where you already have a trusted session.
- **Login walls.** Same answer: use extension mode so the agent works inside your signed-in browser.
- **PDF text.** `save_file` downloads the file, but the agent doesn't read PDF text itself.
