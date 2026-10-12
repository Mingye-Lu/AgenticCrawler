# acrawl MCP — Tool Reference

All 39 MCP tools with exact parameter names, types, enums, and defaults. Parameter names are copied from the live server schemas — use them verbatim.

**Contents**

- [Conventions](#conventions)
- [Navigation (6)](#navigation-6)
- [Interaction (7)](#interaction-7)
- [Extraction (5)](#extraction-5)
- [DevTools & observability (12)](#devtools--observability-12)
- [Device emulation (1)](#device-emulation-1)
- [Script management (7)](#script-management-7)
- [Autonomous agent (1)](#autonomous-agent-1)
- [Not exposed over MCP](#not-exposed-over-mcp)

---

## Conventions

- **Required** parameters are marked. Everything else is optional.
- All schemas are `additionalProperties: false` — an unknown key is an error, not ignored. Typos fail loudly.
- **Tool names are matched verbatim.** A dash form like `read-content` is rejected as an unknown tool (JSON-RPC `-32601`), not normalized. Dash/case normalization exists in the codebase but is applied only to `run_goal`'s `allowed_tools` list.
- The 31 browser tools share **one persistent browser session**. State (cookies, current tab, URL, interception rules) carries across calls.
- Tools returning `page_state`: `click`, `click_at`, `fill_form`, `select_option`, `hover`, `press_key`, `scroll`, `go_back`, `switch_tab`, `set_device`, `refresh`, `wait`.
- The browser launches lazily on the first browser tool call — and on **any** `run_script` call, whether or not the script touches the browser.
- **The agent-loop optimizations do not apply here.** Self-healing, action caching, loop detection, confidence tracking, and budget enforcement all live in `CrawlerAgent::execute`; MCP dispatches straight to the tool registry. They affect `run_goal` only.

---

## Navigation (6)

### `navigate` — **required: `url`**

Loads a URL over HTTP, auto-escalating to headless Chromium when JS rendering is detected (framework markers, auth redirects, 403/429/503, short `<noscript>` bodies, empty SPA shells).

| Param | Type | Default | Notes |
|---|---|---|---|
| `url` | string | — | **Required.** Must include protocol. Relative URLs unsupported. |
| `format` | enum | `fit_markdown` | `markdown` · `text` · `html` · `fit_markdown` |
| `content_depth` | enum | `main` | `full` · `main` · `slim` (first 2000 chars) · `none` |
| `page_map_depth` | enum | `slim` | `full` (includes CSS selectors) · `slim` · `none` |
| `strip_images` | boolean | `true` | Set `false` only when you need image URLs/alt text. |

`content_depth` and `page_map_depth` are independent — set both to `none` for a cheap "just load it" call. Use `format: "markdown"` when target content lives in a sidebar/nav that `fit_markdown` prunes. If `fit_markdown` prunes everything, it falls back to plain text automatically.

### `go_back` — no parameters

Browser back button. Returns `page_state`.

### `refresh` — no parameters

Reloads the current page. **Required after `intercept_network`** to apply rules. Increments `seq`.

### `scroll`

| Param | Type | Default | Notes |
|---|---|---|---|
| `direction` | enum | — | `up` · `down` |
| `pixels` | integer | `500` | 300–800 for a normal scroll; larger to reach the bottom fast. |

Triggers lazy-load/infinite-scroll content.

### `switch_tab`

| Param | Type | Notes |
|---|---|---|
| `index` | integer | Zero-based (0 = first tab). |

### `wait`

| Param | Type | Default | Notes |
|---|---|---|---|
| `selector` | string | — | CSS selector. Mutually exclusive with `seconds`. |
| `seconds` | number | — | Max 300. Mutually exclusive with `selector`. |
| `state` | enum | `attached` | `visible` · `hidden` · `attached` · `detached` |
| `silent` | boolean | `false` | Suppresses `page_state`. Time-only waits only. |

`state: "hidden"` is the idiomatic way to wait out a loading spinner. Use `seconds` + `silent: true` for a cheap pause.

---

## Interaction (7)

### `click`

| Param | Type | Default | Notes |
|---|---|---|---|
| `selector` | string | — | CSS selector or `@eN`. Mutually exclusive with `text`. |
| `text` | string | — | Visible label. Mutually exclusive with `selector`. |
| `role` | string | — | ARIA role filter for `text` (`button`, `tab`, `checkbox`, `menuitem`…). |
| `region` | string | — | `[ref=eN]` or semantic token to constrain label matching. |
| `widen` | boolean | `false` | Full-page diff instead of container-scoped. |

Prefer `text` + `role` in SPAs where generated class names are unstable.

### `click_at` — **required: `x`, `y`**

| Param | Type | Default |
|---|---|---|
| `x` | number | — |
| `y` | number | — |
| `widen` | boolean | `false` |

Viewport pixels. Only for canvas, maps, SVG regions, and other selector-less targets. Get coordinates via `execute_js` + `getBoundingClientRect()`.

### `fill_form` — **required: `fields`**

| Param | Type | Default | Notes |
|---|---|---|---|
| `fields` | object (string→string) | — | Keys: CSS selector, `name`/`id`, `@eN`, **or visible label text**. |
| `submit` | boolean | `false` | Fires form submission after filling. |
| `form_selector` | string | — | Disambiguates when the page has multiple forms. |
| `widen` | boolean | `false` | |

Label resolution is page-wide, so modals and div-based UIs with no `<form>` boundary work. On a submit-triggered redirect, acrawl waits for SPA readiness (DOM ready, visible text, hydration buffer) before returning.

### `select_option` — **required: `selector`**

| Param | Type | Notes |
|---|---|---|
| `selector` | string | Native `<select>` or custom ARIA/portal dropdown trigger. |
| `value` | string | Matched against the `value` attribute (or option text for custom dropdowns). |
| `label` | string | Visible option text. |
| `index` | integer | Zero-based. |
| `widen` | boolean | Default `false`. |

**Discovery mode:** omit `value`, `label`, and `index` to open the dropdown, enumerate available options, and return them without selecting. Do this before guessing.

### `hover` — **required: `selector`**

| Param | Type | Default |
|---|---|---|
| `selector` | string | — |
| `widen` | boolean | `false` |

For tooltips, dropdown menus, hover-revealed content. Use `click` when the element needs activation rather than hover.

### `press_key` — **required: `key`**

| Param | Type | Notes |
|---|---|---|
| `key` | string | Playwright names: `Enter`, `Escape`, `Tab`, `ArrowDown`, `ArrowUp`, `Backspace`, `Space`, single chars. Combos: `Control+a`, `Shift+Tab`. |
| `selector` | string | Focus this element first. Otherwise goes to the focused element/page. |
| `widen` | boolean | Default `false`. |

### `execute_js` — **required: `script`**

| Param | Type | Default | Notes |
|---|---|---|---|
| `script` | string | — | Runs in page context. Last expression's value is JSON-serialized. `await` supported. |
| `hover_selector` | string | — | CSS selector or `@eN` to hover **before** evaluating — lets you read `:hover` computed styles. |
| `settle_ms` | integer | `0` | Max 5000. Wait after execution so React/Vue reactivity flushes before capture. |

Prefer `click`/`fill_form`/`select_option` for standard interactions. Use `execute_js` for bulk extraction (returning an array of objects in one call is far cheaper than N tool calls) and for anything the dedicated tools cannot express.

`settle_ms` exists because a `.click()` inside `execute_js` returns before the framework re-renders — without it you capture pre-mutation state.

---

## Extraction (5)

### `page_map`

| Param | Type | Default | Notes |
|---|---|---|---|
| `scope` | string | — | `[ref=eN]` subtree, or semantic token: `dialog`, `main`, `sidebar`. |
| `depth` | integer | `5` | Max 10. At max depth, omitted children are counted. |

Returns the YAML accessibility tree — the primary structural view. Node form:

```
- role "name" [state...] [ref=eN]:
```

Links are capped at 50; use `list_resources` for the complete set.

### `read_content`

| Param | Type | Default | Notes |
|---|---|---|---|
| `heading` | string | — | Exact heading text, case-insensitive. Captures until the next heading of equal/higher level. |
| `selector` | string | — | Precise DOM target. |
| `offset` | integer | `0` | Character offset. |
| `max_chars` | integer | `10000` | |

If `heading` isn't found, the response lists available headings as a hint. Paginate large sections with `offset` + `max_chars` rather than raising `max_chars` indefinitely.

### `list_resources` — no parameters

All links (href + text), images (src + alt), and forms (action + method). **No caps** — this is the escape hatch for `page_map`'s 50-link limit.

### `screenshot`

| Param | Type | Default | Notes |
|---|---|---|---|
| `selector` | string | — | Element screenshot. Overrides `full_page`. |
| `full_page` | boolean | `false` | Full scrollable height. |
| `format` | enum | `png` | `png` · `jpeg` · `webp` |
| `quality` | integer | `80` | 0–100, jpeg/webp only. |
| `save` | boolean | `false` | Save to disk and return path instead of base64. |
| `filename` | string | — | Only with `save: true`. |
| `output_dir` | string | — | Overrides the default output directory. |

**Last resort.** Screenshots can't be searched or parsed. Exhaust `page_map`, `read_content`, and `execute_js` first.

### `save_file` — **required: `url`**

| Param | Type | Notes |
|---|---|---|
| `url` | string | Fully qualified, includes protocol. |
| `filename` | string | Derived from the URL's last path segment if omitted. `../` is rejected. |
| `subdir` | string | e.g. `images`, `data/csv`. Created automatically. |
| `output_dir` | string | Relative (to CWD) or absolute. |
| `headers` | object (string→string) | e.g. `Referer` for CDNs that require it. |

Downloads any file type via HTTP GET. acrawl cannot read text out of a saved PDF — to get PDF content, navigate to an HTML version or extract externally.

---

## DevTools & observability (12)

These buffer continuously from browser launch and share the `seq` counter. See SKILL.md §5 for the temporal model.

> **`@rN` / `@logN` / `@wsN` are positional within the most recent listing, not stable IDs.** Each list call rebinds them. Call the matching inspector immediately, and don't reuse a ref after listing again with different `filter`/`sort_by`/`since`.

### `list_network_activity`

| Param | Type | Default | Notes |
|---|---|---|---|
| `since` | string \| number | `last` | `all` · `last` · `seq` |
| `until` | string \| number | `now` | `now` · `seq` (exclusive) |
| `filter` | enum | `all` | `all` · `xhr` · `media` · `failed` · `pending` · `aborted` |
| `pattern` | string | — | URL substring. |
| `method` | string | — | Case-insensitive, e.g. `GET`, `POST`. |
| `unique_urls` | boolean | `false` | Collapses repeats; representative row = largest response. Adds `request_count`. |
| `min_size_kb` | integer | — | |
| `max_size_kb` | integer | — | |
| `sort_by` | array of enum | `["oldest"]` | `slowest` · `fastest` · `largest` · `smallest` · `newest` · `oldest`. First is primary, rest are tiebreakers. |
| `limit` | integer | `20` | |

Returns `@rN` refs plus an inline `content_type`.

### `inspect_request` — **required: `id`**

| Param | Type | Default |
|---|---|---|
| `id` | string | — (`@rN`) |
| `include_body` | boolean | `false` |

Returns metadata, coarse timing, initiator type, and notes about headers/bodies that weren't captured.

### `list_page_logs`

| Param | Type | Default | Notes |
|---|---|---|---|
| `level` | enum | `all` | `all` · `error` · `warning` · `info` · `debug` |
| `since` | string \| number | `last` | |
| `until` | string \| number | `now` | |
| `group_by` | enum | `message` | `message` (dedupes, assigns `@logN`) · `source` · `level` |

`group_by: "message"` is what produces the `@logN` IDs that `inspect_log` needs.

### `inspect_log` — **required: `id`**

| Param | Type | Default |
|---|---|---|
| `id` | string | — (`@logN`) |
| `limit` | integer | `5` |

Concrete instances with timestamps, stack traces, source locations.

### `list_websocket_activity`

| Param | Type | Default |
|---|---|---|
| `since` | string \| number | `last` |
| `until` | string \| number | `now` |

Connections + message counts, with `@wsN` refs.

### `inspect_websocket` — **required: `id`**

| Param | Type | Default | Notes |
|---|---|---|---|
| `id` | string | — | `@wsN` |
| `direction` | enum | `all` | `sent` · `received` · `all` |
| `sort_by` | enum | `newest` | `newest` · `oldest` |
| `limit` | integer | `30` | |
| `pattern` | string | — | Substring match on message data. |

### `get_page_performance` — no parameters

Navigation Timing + Resource Timing: TTFB, DOM timings, top 20 resources by transfer size. Works on SPAs.

### `inspect_cookies`

| Param | Type | Default | Notes |
|---|---|---|---|
| `domain` | string | — | Domain substring filter. |
| `issues_only` | boolean | `false` | Only cookies with detected issues. |

Flags `missing_secure`, `missing_httponly`, `sameSite_none_without_secure`, `excessive_lifetime`, `overly_broad_domain`, plus third-party detection.

### `inspect_storage`

| Param | Type | Default | Notes |
|---|---|---|---|
| `target` | enum | `all` | `local` · `session` · `all` |
| `pattern` | string | — | Key substring filter. |

### `measure_coverage`

| Param | Type | Default | Notes |
|---|---|---|---|
| `type` | enum | `all` | `js` · `css` · `all` |
| `reset` | boolean | `false` | Stop in-progress coverage, clear, restart. |

Per-file executed/applied bytes vs total loaded — finds unused bundles and oversized dependencies.

### `audit_accessibility`

| Param | Type | Default | Notes |
|---|---|---|---|
| `scope` | string | — | CSS selector to limit the audit. |
| `standard` | enum | `wcag2aa` | `wcag2a` · `wcag2aa` · `wcag21aa` · `wcag22aa` |
| `impact` | enum | `all` | `critical` · `serious` · `moderate` · `minor` · `all` |

axe-core. Violations grouped by impact, with selectors and descriptions.

### `intercept_network` — **required: `action`**

| Param | Type | Default | Notes |
|---|---|---|---|
| `action` | enum | — | `block` · `mock_response` · `remove_rule` · `clear_all` |
| `pattern` | string | — | Required for `block`/`mock_response`. URL glob; `*` crosses path separators. Prefix `re:` for regex. |
| `mock` | object | — | For `mock_response`. |
| `rule_id` | string | — | For `remove_rule`. |

`mock` fields: `status` (integer, default `200`), `headers` (object), `body` (string), `content_type` (string, default `application/json`).

Rules are **additive** — each call adds one. They apply to future requests, so **call `refresh` afterwards**. Examples: `*ads.com*` blocks any URL containing `ads.com`; `*/api/v2/*` matches that path on any host; `re:api/v[0-9]+` for regex.

---

## Device emulation (1)

### `set_device`

| Param | Type | Notes |
|---|---|---|
| `device` | string | Preset name. Cannot combine with custom fields. |
| `viewport` | object | `{width, height}`, both integers ≥ 1. |
| `userAgent` | string | |
| `deviceScaleFactor` | number | > 0. e.g. 2 retina, 3 iPhone. |
| `isMobile` | boolean | |
| `hasTouch` | boolean | |

Presets: `iphone_15`, `iphone_se`, `iphone_15_pro_max`, `pixel_7`, `galaxy_s24`, `ipad_pro`, `ipad`, `galaxy_tab_s9`, `desktop`, `desktop_hd`.

`device` and the custom fields are mutually exclusive. Recreates the browser context — **cookies and localStorage are preserved**. Cannot be used while sub-agents are running. Use `desktop` to reset. Returns a differential `page_state` showing responsive layout changes.

---

## Script management (7)

Full DSL in `references/scripting.md`.

### `run_script`

| Param | Type | Notes |
|---|---|---|
| `script` | object | Inline definition. **Must include `schema_version: 1`** (integer) and a non-empty `steps`. |
| `name` | string | **Not functional over MCP** — see below. |
| `save_as` | string | **Not functional over MCP** — see below. |
| `limits` | object | All-or-nothing override, not a patch. Five fields are mandatory: `max_steps`, `max_timeout_secs`, `per_step_timeout_secs`, `max_output_bytes`, `max_parallel_branches`. |

Returns `{"script_id": "scr_XXXXXXXX"}` **immediately** — it does not block. Collect with `wait_for_scripts`.

Two options in this schema are not wired up on the MCP path:

- **`name`** is converted to an internal `{"__load_from_disk": "<name>"}` marker that only the agent loop resolves. Over MCP that marker reaches the script parser, which expects `schema_version`/`steps`, so the call fails. To run a saved script from MCP, `read_script` it and pass the JSON as `script`.
- **`save_as`** is parsed into the task but never written to disk by any consumer, so nothing is persisted and `list_scripts` will not show it. Use `save_script` explicitly instead.

Do **not** issue concurrent `run_script` calls for browser work: all scripts share the session's tab and will silently corrupt each other's results (see `references/scripting.md`).

> The tool description says `version ("1.0")`. That is wrong; the server requires `schema_version` as an integer.

### `script_status` — **required: `script_id`**

Non-blocking. Returns the serialized `ScriptState`:

```json
{"script_id":"scr_...","status":"Running","step":3,"total_steps":null,
 "current_url":"https://...","items_collected":7,"elapsed_secs":4.2,
 "errors_caught":0,"yielded_data":[]}
```

Two things to note: `status` values are **capitalized** (`Pending`, `Running`, `Completed`, `Failed`, `Cancelled`), and there is **no `extracted_data` field** — you get `items_collected` as a count plus whatever `yield` produced. To read collected data you must call `wait_for_scripts`. Use `yield` if you need visibility into values mid-run.

### `wait_for_scripts`

| Param | Type | Notes |
|---|---|---|
| `script_ids` | array of string | Omit to wait for **all** active scripts. |

Blocks. Returns an array of results with `extracted_data`, `yielded_data`, `steps_executed`, `elapsed_secs`, `status`, `error`. **Partial data is returned even on failure.**

### `cancel_script` — **required: `script_id`**

Sets a cancellation token and marks the state `Cancelled`. It does **not** abort the task and does **not** close a browser tab. The executor notices only when it reaches its next limit check, so an in-flight tool call runs to completion first — cancellation is cooperative, not immediate.

Already-collected `extracted_data` is **preserved** and still returned by `wait_for_scripts`, so cancelling does not throw away progress.

### `save_script` — **required: `name`, `script`**

Persists to `~/.acrawl/scripts/<name>.json`. Overwrites an existing script with the same name. It checks JSON/schema shape only and does **not** run the semantic validator, so a script with undefined variables or unknown tool names saves fine and fails later at `run_script`. A successful save is a syntax check, not a correctness guarantee.

### `list_scripts` — no parameters

Saved scripts with `name`, `modified_at`, `size_bytes`.

### `read_script` — **required: `name`**

Full JSON definition. Name as shown by `list_scripts`, without `.json`.

---

## Autonomous agent (1)

### `run_goal` — **required: `goal`**

| Param | Type | Default | Notes |
|---|---|---|---|
| `goal` | string | — | Natural-language goal. Max 100,000 chars. |
| `model` | string | `settings.json` `model` | `provider/model-id`, e.g. `anthropic/claude-sonnet-4-6`. |
| `allowed_tools` | array of string | all | Restrict the agent's toolbox. |
| `max_steps` | integer | `50` (hardcoded) | 1-200. `settings.max_steps` is NOT consulted. |

The only tool that needs LLM credentials. Note the two-part requirement: credentials in `~/.acrawl/credentials.json` (via `acrawl auth`) **and** a prefixed `model` in `settings.json` (via `acrawl config set model <provider>/<id>`). An omitted `model` is resolved *only* from `settings.model` — the active provider's `default_model` in `credentials.json` is not consulted, so credentials alone yield `no model configured`.

`max_steps` likewise defaults to a hardcoded `50` and ignores `settings.max_steps`; pass it explicitly to bound the work.

Creates its **own** agent, browser, and API client, so it does **not** share the browser session, cookies, or interception rules with your other MCP calls.

Returns a text summary plus `structuredContent`:

```json
{
  "summary": "...",
  "extracted_data": [],
  "steps_executed": 12,
  "model_used": "anthropic/claude-sonnet-4-6",
  "allowed_tools": [],
  "goal": "..."
}
```

Use it for genuinely adaptive, unknown-shape tasks. Prefer manual tools or a script when you need intermediate control or when the page structure is already known — `run_goal` is opaque while it runs.

`allowed_tools` is a useful safety rail: restricting to read-only tools (`navigate`, `read_content`, `page_map`, `list_resources`) guarantees the delegated agent cannot click, submit, or download anything.

---

## Not exposed over MCP

These 4 agent-control tools exist in the 42-tool internal toolbox but are **excluded** from MCP; calling one returns JSON-RPC `-32601` (method not found):

`fork` · `wait_for_subagents` · `subagent_status` · `cancel_subagent`

For concurrency from MCP, fire several `run_script` calls and join them with `wait_for_scripts` (up to `max_concurrent_scripts`, default 5), or ask `run_goal` to fork sub-agents internally. Do **not** rely on script `parallel` branches — they are broken for 2+ branches; see `references/scripting.md`.

**Arithmetic:** 42 internal = 31 browser + 4 agent-control + 7 script. MCP surface = 42 − 4 excluded + 1 `run_goal` = **39**.
