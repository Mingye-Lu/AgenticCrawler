---
title: "Tools"
description: "All 43 acrawl agent tools by group: navigation, interaction, extraction, DevTools observation, sub-agents and deterministic scripts."
head:
  - tag: title
    content: "acrawl tools: 43 browser, DevTools and script tools"
  - tag: meta
    attrs:
      property: og:title
      content: "acrawl tools: 43 browser, DevTools and script tools"
---

acrawl's agent has **43 tools** in six groups. The MCP server exposes 40 of them: every navigation, interaction, extraction and DevTools tool, the 7 script tools and `run_goal`. The 4 sub-agent tools are used inside the agent loop, so MCP clients reach them through `run_goal`.

Each tool's exact parameters are in its JSON schema, which MCP clients receive when they connect. The tables below say what each one is for.

## Navigation (7)

| Tool | Purpose |
|---|---|
| `navigate` | Open a URL. HTTP first, browser when needed. Returns `markdown`, `fit_markdown`, `text` or `html` plus a `page_map`. |
| `go_back` | Browser back. |
| `get_page_info` | Cheap read of URL, title and ready state; optionally lists all tabs. |
| `scroll` | Scroll by pixels to trigger lazy loading. |
| `switch_tab` | Switch tabs by index. |
| `wait` | Wait for a selector state or a fixed time. |
| `refresh` | Reload, for example after adding intercept rules. |

## Interaction (8)

| Tool | Purpose |
|---|---|
| `click` | Click by CSS selector, `@eN` ref or visible label. |
| `click_at` | Click viewport coordinates (canvas, maps, SVG). |
| `fill_form` | Fill fields by selector, name, ref or label; optional submit. |
| `select_option` | Pick from native or custom dropdowns, or list their options. |
| `hover` | Reveal tooltips and menus. |
| `press_key` | Send keys such as Enter, Escape or Tab. |
| `set_device` | Emulate a phone, tablet or desktop. |
| `execute_js` | Run JavaScript in the page and return the result. |

## Extraction (5)

| Tool | Purpose |
|---|---|
| `page_map` | YAML accessibility tree with `@eN` refs, the main structural view. |
| `read_content` | Text by heading or selector, with pagination. |
| `list_resources` | Links, images and forms on the page. |
| `screenshot` | Full-page PNG. |
| `save_file` | Download a URL to the output folder. |

## DevTools and observation (12)

| Tool | Purpose |
|---|---|
| `list_network_activity` / `inspect_request` | Requests with timing, state, initiator. |
| `list_page_logs` / `inspect_log` | Deduplicated console logs with stack traces. |
| `list_websocket_activity` / `inspect_websocket` | WebSocket connections and frames. |
| `get_page_performance` | TTFB, DOM timings, largest resources. |
| `inspect_cookies` | Cookies with security analysis. |
| `inspect_storage` | localStorage and sessionStorage. |
| `measure_coverage` | Used vs. loaded JS and CSS bytes. |
| `audit_accessibility` | axe-core WCAG audit. |
| `intercept_network` | Block or mock requests by URL pattern. |

## Sub-agents (4)

| Tool | Purpose |
|---|---|
| `fork` | Start a child agent on a new tab with its own goal and step budget. |
| `wait_for_subagents` | Collect results from children. |
| `subagent_status` | Check children without blocking. |
| `cancel_subagent` | Stop a child. |

## Scripts (7)

| Tool | Purpose |
|---|---|
| `run_script` | Run a deterministic script with no per-step LLM calls. |
| `save_script` / `list_scripts` / `read_script` | Store and reuse scripts. |
| `wait_for_scripts` / `script_status` / `cancel_script` | Track and control runs. |

See [Repeatable crawls with zero LLM calls](/AgenticCrawler/docs/guides/deterministic-scripts/).

## Page formats

`navigate` takes a `format` that controls how much of the page comes back:

| Format | What you get |
|---|---|
| `fit_markdown` | Recommended. Markdown with navigation, footers, ads, sidebars and other low-content blocks removed first. Typically 30 to 60% fewer tokens. |
| `markdown` | The whole page as Markdown. |
| `text` | Plain text. |
| `html` | The raw HTML. |

`fit_markdown` drops elements whose class or id names them as navigation, footer, sidebar, ads, comments or sharing widgets, then scores what is left by text density, link density and tag, and keeps the content-heavy parts. Use `markdown` when the data you want lives in a sidebar or similar block. If pruning would leave nothing, the tool returns plain text instead.

## What interaction tools return

Tools that change the page (`click`, `click_at`, `fill_form`, `select_option`, `hover`, `press_key`, `scroll`, `go_back`, `switch_tab`, `set_device`, `refresh`) return a `page_state`, so the agent sees the effect of its action without a separate call.

The first action on a URL returns the full accessibility tree, with a `ref` on every element the agent can use:

```yaml
url: https://example.com/page
title: Page Title
page_map: |
  - main "" [ref=e1]:
    - navigation "Primary" [ref=e2]:
      - link "Docs" [ref=e3]:
    - heading "Platform" [level=1] [ref=e4]:
    - button "Get Started" [ref=e5]:
```

Later actions on the same page return only what changed, which keeps long sessions cheap:

```yaml
url: https://example.com/page
title: Page Title
changed: true
changes: |
  + [ref=e5] added:
    - dialog "Confirm" [ref=e5]:
      - button "OK" [ref=e6]:
      - button "Cancel" [ref=e7]:
  ~ button "Submit" [ref=e2]: [] → [disabled]
```

If nothing changed, the result is just `changed: false`. When the changes are too large to diff usefully, the full tree comes back instead.
