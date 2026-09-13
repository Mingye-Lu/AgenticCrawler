# acrawl MCP — Recipes

End-to-end workflows. Outputs shown were captured from live runs against real sites.

**Contents**

- [1. Explore an unknown page cheaply](#1-explore-an-unknown-page-cheaply)
- [2. Scrape a listing page](#2-scrape-a-listing-page)
- [3. Crawl many pages](#3-crawl-many-pages)
- [4. Log in / submit a form](#4-log-in--submit-a-form)
- [5. Find what's slow](#5-find-whats-slow)
- [6. Find failing requests](#6-find-failing-requests)
- [7. Debug console errors](#7-debug-console-errors)
- [8. Accessibility sweep](#8-accessibility-sweep)
- [9. Block or mock a request](#9-block-or-mock-a-request)
- [10. Cookie & storage review](#10-cookie--storage-review)
- [11. Find unused JS/CSS](#11-find-unused-jscss)
- [12. Responsive check](#12-responsive-check)
- [13. Delegate to run_goal safely](#13-delegate-to-run_goal-safely)

---

## 1. Explore an unknown page cheaply

Load structure without prose, then decide what to read.

```
navigate(url: "...", content_depth: "none", page_map_depth: "slim")
```

Response is small and includes a `seq` you can use as a temporal anchor later:

```json
{"content":"","content_depth":"none","seq":0,
 "title":"All products | Books to Scrape - Sandbox","url":"https://books.toscrape.com/"}
```

Then read only what you need:

- `page_map(scope: "main", depth: 3)` — narrow the structural view
- `read_content(heading: "...")` or `read_content(selector: ".product_pod")` — targeted text
- `list_resources()` — the complete link/image/form set when `page_map`'s 50-link cap bites

Escalate to `content_depth: "main"` only once you know the prose is what you want.

---

## 2. Scrape a listing page

One `execute_js` returning an array of objects costs a single round-trip and gives you clean structured data. This beats mapping the page and issuing per-item calls.

```
navigate(url: "https://books.toscrape.com/", content_depth: "none", page_map_depth: "none")

execute_js(script: "Array.from(document.querySelectorAll('article.product_pod')).map(el => ({ title: el.querySelector('h3 a')?.title, price: el.querySelector('.price_color')?.innerText, inStock: !!el.querySelector('.instock.availability'), url: el.querySelector('h3 a')?.href }))")
```

Use optional chaining (`?.`) so one malformed row yields `null` instead of throwing and losing the whole batch.

If the list is lazy-loaded, `scroll(direction: "down", pixels: 800)` until the count stops growing, then extract once.

---

## 3. Crawl many pages

Prove the extraction on one page first (recipe 2). Then move the loop into a script so the remaining pages cost zero LLM round-trips.

Enumerate URLs as literals — `$var` substitution **cannot** build strings, so `"?page=$n"` will not interpolate (see `scripting.md`).

```json
{
  "schema_version": 1,
  "steps": [
    {"type": "assign", "variable": "urls", "value": {"kind": "literal", "value": [
      "https://books.toscrape.com/catalogue/page-1.html",
      "https://books.toscrape.com/catalogue/page-2.html"
    ]}},
    {"type": "for_each", "variable": "u",
      "iterable": {"kind": "variable", "value": "urls"},
      "steps": [
        {"type": "tool_call", "tool": "navigate",
          "input": {"url": "$u", "content_depth": "none", "page_map_depth": "none"}},
        {"type": "tool_call", "tool": "execute_js",
          "input": {"script": "Array.from(document.querySelectorAll('article.product_pod')).map(el => ({ title: el.querySelector('h3 a')?.title, price: el.querySelector('.price_color')?.innerText }))"},
          "output": "rows"},
        {"type": "collect", "value": {"kind": "variable", "value": "rows"}},
        {"type": "yield", "value": {"kind": "literal", "value": "$u"}}
      ]}
  ]
}
```

Then `wait_for_scripts(script_ids: ["scr_..."])`.

**Run one script at a time.** Concurrent `run_script` calls all share the session's single tab and silently return data from each other's pages, and `parallel` branches fail teardown with 2+ branches. If you need real parallelism, delegate to `run_goal` — its sub-agents each get their own tab. See `references/scripting.md`.

Long crawl? `yield` after each page and poll `script_status` for progress without blocking. Note `script_status` reports `items_collected` as a count and does not expose collected values, so `yield` is what makes intermediate data visible.

---

## 4. Log in / submit a form

```
navigate(url: "https://example.com/login")
page_map(scope: "main")
fill_form(fields: {"Email": "user@example.com", "Password": "..."}, submit: true)
```

Keys resolve by **visible label text** page-wide, so modals and div-based forms with no `<form>` element work. Fall back to `@eN` refs or CSS if labels are ambiguous, and use `form_selector` when the page has several forms.

After submit, read the returned `page_state`:

- `changed: true` with new content → proceed.
- `changed: false` → **do not resubmit.** Check `list_page_logs(level: "error")` for client-side validation, and confirm the fields actually received values.

acrawl waits for SPA readiness after a submit-triggered redirect, so an immediate follow-up read is usually safe. For slow hydration, `wait(selector: "<something only the logged-in page has>", state: "visible")`.

For login-walled sites over MCP, you must authenticate inside the MCP session itself (`fill_form`), because the MCP server always drives its own CloakBrowser and cannot inherit your real browser's cookies. Extension mode is a REPL feature, not an MCP one. acrawl cannot receive SMS codes or generate TOTP.

---

## 5. Find what's slow

```
navigate(url: "...")
get_page_performance()
```

Returns navigation timings plus the top 20 resources by transfer size:

```json
{"navigation":{"ttfb_ms":38,"dom_interactive_ms":594,"dom_complete_ms":607,
               "load_event_ms":608,"transfer_size_bytes":5576,"decoded_size_bytes":51294},
 "summary":{"total_requests":20,"total_transfer_kb":284.5,
            "largest":{"size_kb":42.8,"url":".../fontawesome-webfont.woff"},
            "slowest":{"duration_ms":122,"url":".../27a53d0bb95bdd88288eaf66c9230d7e.jpg"}}}
```

For request-level triage, sort by adjective — primary first, rest as tiebreakers:

```
list_network_activity(since: "all", sort_by: ["slowest", "largest"], limit: 10)
```

`unique_urls: true` collapses repeats (keeping the largest response as representative) so a polling endpoint doesn't crowd out everything else. Note `decoded_size_kb` far exceeding `transfer_size_kb` just means good compression — chase large *decoded* sizes for parse cost, large *transfer* sizes for bandwidth.

---

## 6. Find failing requests

```
list_network_activity(since: "all", filter: "failed")
```

Real output from `books.toscrape.com` — one broken dependency the page itself never surfaces:

```json
{"requests":[{"id":"@r1","method":"GET","state":"failed","status":null,"type":"script",
              "url":"http://ajax.googleapis.com/ajax/libs/jquery/1.9.1/jquery.min.js"}],
 "summary":{"failed":1,"total":1}}
```

An `http://` script on an `https://` page — blocked as mixed content. This is exactly the class of bug that's invisible from the rendered DOM.

`inspect_request(id: "@r1")` adds initiator, timing, and header/body availability notes.

**Refs rebind on every listing.** `@r1` means "first row of the most recent `list_network_activity` response." Inspect immediately after listing; never carry a ref across two listings with different `filter`/`sort_by`.

Also worth checking: `filter: "pending"` (hung requests) and `filter: "aborted"` (cancelled, often by navigation).

---

## 7. Debug console errors

```
list_page_logs(level: "error", group_by: "message")
```

Grouping by message dedupes a noisy loop into one row per distinct error and assigns `@logN`. Then get instances for the one that matters:

```
inspect_log(id: "@log1", limit: 5)
```

That returns timestamps, stack traces, and source locations — enough to attribute the error to a file and line.

To attribute errors to a specific action, note the `seq` before acting, then query `since: <seq>`. `since: "last"` (the default) already scopes to your most recent action, which is usually what you want.

---

## 8. Accessibility sweep

```
audit_accessibility(standard: "wcag21aa", impact: "all")
```

A clean page returns:

```json
{"summary":{"critical":0,"serious":0,"moderate":0,"minor":0,"passes":16,"total_violations":0},
 "violations":[]}
```

Triage strategy: run `impact: "critical"` first for a small, actionable list, then widen. Use `scope: "#main-content"` to audit one region and keep output focused — auditing a whole page with `impact: "all"` on a complex site produces a lot.

Re-audit after interacting: modals and dynamically inserted content have their own violations that a load-time audit never sees.

---

## 9. Block or mock a request

Rules are additive and apply to **future** requests, so a reload is mandatory:

```
intercept_network(action: "block", pattern: "*analytics*")
refresh()
list_network_activity(since: "last")
```

Skipping `refresh` is the usual reason interception "does nothing."

Mock an API to test how the UI handles a payload:

```
intercept_network(action: "mock_response", pattern: "*/api/user*",
                  mock: {status: 200, content_type: "application/json",
                         body: "{\"name\":\"Test User\",\"plan\":\"enterprise\"}"})
refresh()
```

Error-path testing is the same shape with `mock: {status: 500, body: "{}"}` — a fast way to see whether the UI degrades gracefully.

Patterns: `*` crosses path separators (`*ads.com*` matches any URL containing `ads.com`; `*/api/v2/*` matches that path on any host). Prefix `re:` for regex (`re:api/v[0-9]+`).

Clean up with `intercept_network(action: "clear_all")` — rules persist for the session and will silently affect later work.

---

## 10. Cookie & storage review

```
inspect_cookies(issues_only: true)
inspect_storage(target: "all")
```

`issues_only` filters to cookies with detected problems: `missing_secure`, `missing_httponly`, `sameSite_none_without_secure`, `excessive_lifetime`, `overly_broad_domain`. Third-party cookies are flagged too, which makes this a quick tracker inventory.

`inspect_storage` covers localStorage and sessionStorage with sizes; `pattern` filters by key substring. Useful for spotting tokens or PII persisted client-side.

---

## 11. Find unused JS/CSS

```
navigate(url: "...")
measure_coverage(type: "all")     # starts tracking; returns empty
refresh()                         # or exercise the feature
measure_coverage(type: "all")     # now returns real per-file data
```

**The first call always returns empty.** The handler stops any existing session (there is none yet), reports that empty result, and only then starts tracking. So a bare `navigate` → `measure_coverage` sequence tells you nothing — you must measure a second time after the page has loaded or been exercised under an active session.

Once primed, the result gives per-file executed/applied bytes versus total loaded. High unused percentages point at bundles that could be code-split or dropped.

`reset: true` adds an extra stop before that sequence, which is the clean way to discard a previous session and begin fresh. To separate "unused on load" from "genuinely dead", prime coverage, exercise the feature, then measure.

---

## 12. Responsive check

```
set_device(device: "iphone_15")
page_map()
set_device(device: "desktop")
```

Presets: `iphone_15`, `iphone_se`, `iphone_15_pro_max`, `pixel_7`, `galaxy_s24`, `ipad_pro`, `ipad`, `galaxy_tab_s9`, `desktop`, `desktop_hd`. Or supply `viewport` / `userAgent` / `deviceScaleFactor` / `isMobile` / `hasTouch` — but not alongside `device`.

The context is recreated while **cookies and localStorage are preserved**, so a logged-in session survives the switch. Returns a differential `page_state` showing what the breakpoint changed, which is cheaper to read than two screenshots. Reset with `device: "desktop"`.

---

## 13. Delegate to `run_goal` safely

Use it when the page shape is genuinely unknown and the task needs adaptation:

```
run_goal(goal: "Find the pricing page on example.com and extract every tier with its
                monthly price and included features.",
         max_steps: 30)
```

Two guardrails worth applying by default:

- **`max_steps`** (1–200) bounds the work. An underspecified goal with a high budget is how you get a surprising bill.
- **`allowed_tools`** restricts the toolbox. For extraction, `["navigate", "read_content", "page_map", "list_resources"]` makes it structurally impossible for the delegated agent to click, submit, or download anything.

Returns a text summary plus `structuredContent` with `extracted_data`, `steps_executed`, `model_used`, and the resolved `allowed_tools`.

`run_goal` builds its **own** browser and agent — it does not see your current tab, cookies, or interception rules, and its work leaves your session untouched. It is also the only tool needing `~/.acrawl/credentials.json`; if that's unconfigured it fails, and the fix is to configure credentials or switch to manual tools, not to retry.

Prefer manual tools or a script when you already know the structure — `run_goal` is opaque while it runs and you cannot course-correct mid-flight.
