---
name: acrawl-mcp
description: Drive the acrawl MCP server for browser automation, web scraping, and live DevTools inspection — navigating pages, clicking, filling forms, extracting structured data, and auditing network traffic, console errors, cookies, storage, performance, accessibility, and code coverage. Use this whenever a task touches acrawl tools (navigate, page_map, click, fill_form, execute_js, run_script, run_goal) or asks to scrape a site, crawl many pages, fill or submit a web form, log into a site, find what network requests a page makes, debug console errors on a live page, measure page load performance, audit a page for WCAG issues, inspect cookies or localStorage, block or mock requests, or automate a repetitive multi-page browser workflow. Also use it when deciding between manual tool calls, a deterministic script, and autonomous run_goal, or when acrawl calls fail with schema_version, variable-substitution, unsupported-tool, or stale-@ref errors.
---

# acrawl MCP

acrawl exposes **39 MCP tools**: 31 browser/DevTools tools, 7 script tools, and 1 autonomous agent (`run_goal`). All 39 share **one persistent browser session** across calls — except `run_goal`, which builds its own isolated agent and browser.

What makes acrawl different from other browser MCPs is the observability half: you can read network traffic, console logs, WebSocket frames, cookies, storage, coverage, and axe-core audits from a live page. Reach for those instead of guessing why a page misbehaves.

Tool names below are bare (`navigate`). Your client may namespace them (`acrawl_navigate`, `mcp__acrawl__navigate`) — use whatever prefix your tool list shows.

---

## 1. Pick an execution mode first

Three ways to get work done. Choosing wrong is the most expensive mistake available, because it either burns tokens on a loop that should have been a script, or spends a script debugging cycle on a one-off click.

| Mode | Use when | Cost |
|---|---|---|
| **Manual tool calls** | Exploring an unfamiliar page; <10 actions; you need to *see* each result to decide the next step | 1 LLM round-trip per action |
| **`run_script`** | Same operation over 3+ pages/items, and the page structure is already known | **Zero** LLM round-trips during execution |
| **`run_goal`** | Whole task is delegable in one sentence and you don't need intermediate control | 1 call; agent burns its own tokens internally |

The practical pattern: **explore manually until the shape is known, then script the repetition.** Do one page by hand, confirm your selectors, then turn the loop into a script. A 50-page crawl becomes one tool call instead of 150.

Reach for `run_goal` when the target is genuinely unknown and adaptive ("find the pricing page and extract every tier"). It needs `~/.acrawl/credentials.json` configured; the other 38 tools do not. If credentials are missing, `run_goal` fails and the correct move is manual tools or a script, not repeated retries. See `references/setup.md`.

---

## 2. The core loop: observe, act, verify

```
navigate → page_map → act (click/fill/select) → read the returned page_state → repeat
```

Every interaction tool (`click`, `click_at`, `fill_form`, `hover`, `press_key`, `select_option`, `scroll`, `go_back`, `switch_tab`, `set_device`, `refresh`) returns a `page_state` for free. **Read it instead of calling `page_map` again** — that's a wasted round-trip and a wasted page render.

`page_state` arrives in one of three shapes:

- **Full** — first interaction on a URL, or when changes are too broad to diff.
- **Diff** — `changed: true` plus a `changes` block using `+` added / `-` removed / `~` state-changed. This is the common case and it is small; trust it.
- **No-op** — `changed: false`. Your action did nothing. Do **not** retry the identical call; the selector matched nothing meaningful, the element was disabled, or a guard blocked it. Re-map the region and pick a different target.

A `changed: false` after a form submit deserves suspicion: acrawl reports a `CaptchaDetected` error for likely-silent reCAPTCHA v3 rejections, but a plain no-op can also mean client-side validation failed. Check `list_page_logs` for the reason rather than resubmitting.

---

## 3. Token discipline

This is where acrawl sessions go wrong. A full `navigate` on a content-heavy page plus a deep `page_map` can dominate your context in two calls. Defaults are already tuned for economy — the failure mode is overriding them upward "just in case."

**`navigate` gives you two independent dials.** They compose, and both matter:

- `content_depth`: `main` (default) · `full` · `slim` (first 2000 chars) · `none`
- `page_map_depth`: `slim` (default, omits CSS selectors) · `full` · `none`

Set `content_depth: "none"` whenever you only need structure to click through — you get the map without the prose. Set `page_map_depth: "none"` when you only want text. Setting both to `none` makes `navigate` a cheap "just load it" call, which is exactly what you want mid-script.

**`format` controls prose conversion.** `fit_markdown` (default) prunes boilerplate for roughly 30–60% savings by removing elements whose `class`/`id` contains `nav`, `footer`, `header`, `sidebar`, `ads`, `comment`, `promo`, `advert`, `social`, `share`, then score-pruning the rest. Switch to `markdown` when the content you actually want lives in a sidebar or nav — author bios, metadata panels, related links. That's the one case where the default actively destroys your target.

**Other habits that pay:**

- Prefer `read_content` with a `heading` or `selector` over re-navigating. It paginates via `offset`/`max_chars` (default 10000), so pull large sections in slices.
- Scope `page_map` with `scope` (`[ref=eN]`, or a semantic token like `dialog`/`main`/`sidebar`) and keep `depth` shallow. Default depth 5 is usually plenty; max is 10.
- `wait` with `seconds` and `silent: true` skips the page_state diff — use it for pure pauses.
- **`screenshot` is a last resort.** It cannot be searched, diffed, or parsed, and it is expensive. Text tools answer almost every question you would take a screenshot to answer. Use it for genuine visual verification only.
- `list_resources` returns *all* links/images/forms with no cap — it exists precisely because `page_map` truncates links at 50. Use it when you need the complete set, not as a general explorer.

---

## 4. The `@ref` system

acrawl hands out short opaque handles instead of making you carry selectors or URLs. Four families, each produced by a lister and consumed by an inspector:

| Ref | Produced by | Consumed by |
|---|---|---|
| `@eN` | `page_map`, any `page_state` | `click`, `fill_form`, `hover`, `select_option`, `press_key`, `execute_js` (`hover_selector`), `page_map` (`scope`) |
| `@rN` | `list_network_activity` | `inspect_request` |
| `@logN` | `list_page_logs` (with `group_by: "message"`) | `inspect_log` |
| `@wsN` | `list_websocket_activity` | `inspect_websocket` |

The list→inspect split is deliberate: listing stays cheap and scannable, and you pay for detail only on the one item you care about. Don't ask for detail on everything.

**`@eN` refs go stale.** They are tied to a page render. After a navigation or a substantial DOM change, re-read them from the fresh `page_state` or `page_map`. A stale `@eN` is a common cause of a mystifying `changed: false`.

**`@rN` / `@logN` / `@wsN` refs are rebound on every list call.** They are positional within the *most recent* listing, not stable global IDs — despite the `list_network_activity` description saying "stable @rN refs." Verified: `@r1` was a font file under `sort_by: ["largest"]`, then became a failed script under `filter: "failed"`, and `inspect_request(@r1)` returned the latter. So inspect immediately after the listing that produced the ref, and never carry a ref across two listings with different `filter`/`sort_by`/`since`.

**When CSS paths are fragile, target by label instead.** `click` accepts `text` (optionally narrowed by `role` and `region`) and `fill_form` resolves field identifiers by visible label text page-wide — including inside modals and div-based UIs with no `<form>` element. This is usually more durable than a generated class path in an SPA admin panel.

`select_option` has a discovery mode worth knowing: call it with only `selector` and no `value`/`label`/`index`, and it opens the dropdown and enumerates the available options without selecting anything. Use it before guessing.

---

## 5. Temporal filtering for observability

The DevTools tools buffer continuously from browser launch and share a **`seq` counter** that increments on every action. This is what makes "what did *this click* do to the network?" answerable.

`list_network_activity`, `list_page_logs`, and `list_websocket_activity` all take:

- `since`: `"last"` (default — since your last action) · `"all"` (whole session) · a numeric `seq`
- `until`: `"now"` (default) · a numeric `seq`

`since: "last"` is the default because attributing effects to the action you just took is the common question. Use `since: "all"` to retrieve requests from before you started looking — capture begins at browser launch, so early page loads are still there.

**Numeric `since` is inclusive**, and an action's observations carry the same `seq` that action's own response returns — the window is `[since, until)`. So to isolate one interaction, perform it **first**, then query with the `seq` it just returned. Anchoring on the *previous* action's seq sweeps that action's observations in too.

Verified: two navigations returned `seq: 0` then `seq: 1`; `since: 0` listed 64 requests including `https://example.com/`, while `since: 1` listed 63 and excluded it.

`list_network_activity` sorts by adjective — `sort_by: ["slowest", "largest"]` (first is primary, rest are tiebreakers) — and filters by `filter` (`xhr`/`media`/`failed`/`pending`/`aborted`), `pattern`, `method`, and size bounds. `unique_urls: true` collapses repeats, keeping the largest response as representative. That combination answers "what is making this page slow" in a single call.

**Interception requires a reload to take effect.** `intercept_network` rules are additive and apply to *future* requests, so the sequence is: add rule(s) → `refresh` → observe. Skipping the refresh is the usual reason a block appears to do nothing. Clean up with `action: "clear_all"`.

---

## 6. Verified gotchas

These are confirmed against a live server and the source, and several contradict the surrounding documentation. They cause hard failures, so they're worth reading once before writing a script.

**1. `run_script` requires `schema_version: 1` — not `version: "1.0"`.** The tool description is wrong. The server rejects the documented form:

```
Error: invalid script: failed to deserialize script JSON: missing field `schema_version`
```

`schema_version` is an integer, at the script root. Same for `save_script`.

**2. `$var` substitution is whole-string only.** There is no string interpolation. A string is replaced only if it is *entirely* `$name`:

| In a `literal` | Result |
|---|---|
| `"$v"` | value of `v` |
| `"hello_$v"` | `"hello_$v"` — unchanged |
| `"page=$n"` | `"page=$n"` — unchanged |

So you **cannot** build a URL as `"https://x.com?page=$n"` — that string is sent literally. Substitution recurses through arrays and objects, and it preserves type: `"$n"` where `n` is `7` yields the number `7`, not `"7"`.

**3. `js_eval` receives no DSL variables at all.** Its string is forwarded to the page verbatim, and the variable map is never exposed to page scope, so `$n` arrives in JS as a literal `$n`. Combined with gotcha 2, there is **no supported way to build a dynamic URL from a loop variable inside a script.** Enumerate the URLs in your client when you construct the script and iterate them with `for_each` — see `references/scripting.md`.

**4. Scripts can only call 17 of the 31 browser tools.** The scriptable set is `navigate`, `click`, `click_at`, `fill_form`, `page_map`, `read_content`, `screenshot`, `go_back`, `scroll`, `wait`, `select_option`, `execute_js`, `hover`, `press_key`, `switch_tab`, `list_resources`, `save_file`.

Every DevTools tool is excluded, plus `refresh` and `set_device`. **You cannot script a network/console/performance/a11y audit** — drive those with manual calls, or have a script gather pages and inspect afterwards.

**5. `steps_executed` counts only `tool_call` nodes.** `assign`, `collect`, `yield`, and control-flow nodes are free against `max_steps` (default 200), so a computation-only script finishes in about a millisecond. It does **not** avoid a browser launch: over MCP, `run_script` initializes the browser before it even parses the script, so any `run_script` call requires Node and a working Chromium.

**6. Three optimizations default ON**, despite README text saying everything defaults off: `failure_classification`, `self_healing`, and `content_aware_profiles`. But **self-healing only applies to `run_goal`** — it lives in the agent loop, and direct MCP tool calls and script `tool_call` nodes bypass it entirely (see gotcha 9).

**7. The 4 sub-agent tools are not on MCP.** `fork`, `wait_for_subagents`, `subagent_status`, and `cancel_subagent` are internal to the agent loop and return JSON-RPC `-32601` if called.

**8. There is no safe way to run browser scripts concurrently over MCP.** Both available mechanisms are broken, in different ways:

*`parallel` branches* fail during page teardown with 2+ branches. Bodies execute, then the script fails and `extracted_data` comes back empty:

```
failed to close parallel pages: page 3: close_page_failed: Invalid page index 3
```

*Concurrent `run_script` calls* are worse — they fail **silently**. The MCP server clones one `BrowserContext`, preserving its `page_index`, and never allocates a new page, so every concurrent script drives the **same tab** and navigates out from under the others. Verified: script A loaded `example.com` and asked for its title, but received script B's page, and still reported `Completed` with `error: null`:

```json
{"expected":"Example Domain",
 "got":{"result":"All products | Books to Scrape - Sandbox"}}
```

**Run browser scripts one at a time.** For real parallelism use `run_goal`, whose sub-agents each get their own tab. Details in `references/scripting.md`.

**9. Self-healing, action caching, loop detection, and budget enforcement do not apply to MCP tool calls or scripts.** All of it lives in `CrawlerAgent::execute`; the MCP server dispatches straight to the tool registry. These features reach `run_goal` only.

**10. Several documented conveniences are unimplemented on the MCP path.** `run_script(name: ...)` fails instead of loading a saved script; `save_as` never writes a file; tool names are matched verbatim so `read-content` is rejected rather than normalized. See `references/tools.md`.

---

## 7. Failure recovery

Match the response to the signal rather than retrying blindly:

- **`changed: false`** — the action was a no-op. Re-map, pick a different target. Don't repeat it.
- **Selector not found** — nothing retried it for you (self-healing is `run_goal`-only). Get a fresh `page_map` and target by `text`/`role` instead of a CSS path.
- **Empty or shell-like content** — the page is client-rendered and the HTTP tier returned the shell. `wait` for a real selector, then `read_content`. `navigate` auto-escalates to a browser on framework markers, but a slow hydration still needs an explicit wait.
- **CAPTCHA / bot wall** — headless stealth has limits. The remedy is a visible browser (`acrawl config set headless false`, then restart the server). Extension mode is **not** available to MCP calls. Retrying headless will not help.
- **Login-walled content** — hard over MCP: the MCP server always drives its own CloakBrowser instance, so it cannot inherit your real browser's session. Either authenticate within the MCP session via `fill_form`, or drive the site from the acrawl REPL in extension mode instead. acrawl cannot receive SMS or generate TOTP codes.
- **Script failed mid-run** — `wait_for_scripts` still returns `extracted_data` and `yielded_data` accumulated before the failure. Read the partial result before re-running; `yield` exists so progress survives a crash.

---

## Reference files

Read these when you need the detail; don't preload them.

- **`references/tools.md`** — all 39 tools, exact parameter names, types, enums, defaults. Check here before guessing a parameter name.
- **`references/scripting.md`** — script DSL: every node type, expression kinds, limits, and runnable verified examples. Read before writing any script.
- **`references/recipes.md`** — end-to-end workflows: paginated scrape, form login, network/performance audit, a11y sweep, mock-an-API.
- **`references/setup.md`** — install, MCP client config, `credentials.json` / `settings.json`, troubleshooting.
