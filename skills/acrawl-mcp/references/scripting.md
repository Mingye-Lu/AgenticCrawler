# acrawl MCP — Script DSL

Deterministic multi-step automation with **zero LLM round-trips during execution**. You submit a JSON program; acrawl runs it on a cloned browser tab and hands back collected data.

Every example here was executed against a live server and its output recorded. Where behaviour differs from the tool descriptions, this file is correct.

**Contents**

- [Root shape](#root-shape)
- [Lifecycle](#lifecycle)
- [Node types](#node-types)
- [Expressions](#expressions)
- [Variable substitution](#variable-substitution)
- [Static validation](#static-validation)
- [The 17 scriptable tools](#the-17-scriptable-tools)
- [Limits](#limits)
- [Concurrency is unsafe over MCP](#concurrency-is-unsafe-over-mcp)
- [Replaying a saved script without an agent](#replaying-a-saved-script-without-an-agent)
- [Verified examples](#verified-examples)

---

## Root shape

```json
{
  "schema_version": 1,
  "name": "optional_label",
  "steps": [
    {"type": "collect", "value": {"kind": "literal", "value": "hello"}}
  ]
}
```

| Field | Type | Required |
|---|---|---|
| `schema_version` | integer, must be `1` | **yes** |
| `name` | string | no |
| `steps` | array of nodes, **must be non-empty** | **yes** |

**`schema_version` is an integer and it is mandatory.** Omitting it produces:

```
Error: invalid script: failed to deserialize script JSON: missing field `schema_version`
```

An empty `steps: []` is also rejected, with `ValidationError::EmptySteps` ("empty steps in top-level script steps"), so there is no valid do-nothing script.

---

## Lifecycle

`run_script` returns immediately with a handle; it does not block:

```json
{"script_id": "scr_66232fa9"}
```

Collect with `wait_for_scripts` (blocking) or poll with `script_status` (non-blocking):

```json
[{
  "script_id": "scr_66232fa9",
  "status": "Completed",
  "extracted_data": ["WORLD"],
  "yielded_data": [],
  "steps_executed": 0,
  "elapsed_secs": 0.0015222,
  "error": null
}]
```

**Partial data survives failure.** A script that dies mid-run still returns everything collected and yielded up to that point, plus `error`. Always read the partial result before re-running — you may already have most of what you needed.

`steps_executed` counts **only `tool_call` nodes**. `assign`, `collect`, `yield`, and all control flow are free against `max_steps`, so a computation-only script completes in about a millisecond.

That speed does **not** mean no browser was involved. Over MCP, `run_script` initializes the browser *before* parsing the script, so every call requires Node and a working Chromium regardless of content. The `elapsed_secs` field measures only executor time and excludes that launch.

---

## Node types

Discriminated by `"type"` in `snake_case`.

### `tool_call`

```json
{"type": "tool_call", "tool": "navigate", "input": {"url": "https://example.com"}, "output": "page"}
```

| Field | Type | Required |
|---|---|---|
| `tool` | string — must be in the [17 allowed tools](#the-17-scriptable-tools) | yes |
| `input` | object — the tool's arguments | yes |
| `output` | string — variable to bind the result to | no |

### `assign`

```json
{"type": "assign", "variable": "v", "value": {"kind": "literal", "value": "WORLD"}}
```

### `collect`

```json
{"type": "collect", "value": {"kind": "variable", "value": "v"}}
```

Appends to `extracted_data` — your primary output.

### `yield`

```json
{"type": "yield", "value": {"kind": "literal", "value": "page 3 done"}}
```

Appends to `yielded_data`, which is readable **while the script is still running** via `script_status`. Two reasons to use it: progress reporting on a long crawl, and durability — yielded data survives `cancel_script`, whereas unyielded partial results are discarded on cancel.

### `for_loop`

```json
{
  "type": "for_loop", "variable": "i",
  "from": {"kind": "literal", "value": 0},
  "to": {"kind": "literal", "value": 3},
  "steps": [{"type": "collect", "value": {"kind": "literal", "value": "$i"}}]
}
```

**`to` is exclusive.** Verified: `from: 0, to: 3` collected `[0, 1, 2]`.

### `for_each`

```json
{
  "type": "for_each", "variable": "item",
  "iterable": {"kind": "variable", "value": "arr"},
  "steps": []
}
```

Over an **array**, `item` is each element. Over an **object**, each iteration binds `{"key": ..., "value": ...}` — verified: iterating `{"k1":1,"k2":2}` yielded `{"key":"k1","value":1}` then `{"key":"k2","value":2}`. Reach into it with `field_access` on `key`/`value`.

### `while_loop`

```json
{"type": "while_loop", "condition": {"kind": "variable", "value": "flag"}, "steps": []}
```

The condition is re-evaluated before each iteration. Because the DSL has **no arithmetic** (see [Expressions](#expressions)), you cannot increment a counter with `assign` alone — drive the condition from a `js_eval` or from a `tool_call` result. Prefer `for_loop` whenever the bound is known; it's simpler and can't run away.

### `if_else`

```json
{
  "type": "if_else",
  "condition": {"kind": "literal", "value": 0},
  "then_steps": [],
  "else_steps": []
}
```

`else_steps` is optional. Falsy: `null`, `false`, `0`, `""`, `[]`, `{}`. Verified: condition `0` took the else branch.

### `try_catch`

```json
{
  "type": "try_catch",
  "try_steps": [],
  "catch_steps": [],
  "finally_steps": [],
  "error_var": "e"
}
```

Only `try_steps` is required. `finally_steps` always runs. `error_var` binds the error message for use inside `catch_steps`.

This catches **tool failures** — a missing selector, a navigation error — and a step that exceeds `per_step_timeout_secs` (`error_var` is then `"script step timed out"`). It does **not** catch undefined variables in `variable` expressions (those are rejected at parse time, see [Static validation](#static-validation)), and it cannot catch step-limit, wall-clock timeout, or cancellation.

### `parallel`

```json
{"type": "parallel", "branches": [[], []]}
```

`branches` is an array of step-arrays. Each branch does get its own browser page, but teardown is broken for 2+ branches — see [Concurrency is unsafe over MCP](#concurrency-is-unsafe-over-mcp).

---

## Expressions

Every value position takes an expression tagged by `"kind"`, with the payload under `"value"`.

| Kind | Shape | Result |
|---|---|---|
| `literal` | `{"kind":"literal","value":<any JSON>}` | the value, **after `$var` / `{{var}}` substitution** |
| `variable` | `{"kind":"variable","value":"name"}` | the variable's value |
| `js_eval` | `{"kind":"js_eval","value":"document.title"}` | JS result from the page — **no substitution** |
| `field_access` | `{"kind":"field_access","value":{"object":<expr>,"field":"k"}}` | property, or `null` if absent |
| `array_index` | `{"kind":"array_index","value":{"array":<expr>,"index":<expr>}}` | element, or `null` if out of bounds |

Verified: `array_index` on `["a","b"]` with index `1` returned `"b"`; `field_access` on `{"k1":1,"k2":2}` with field `k2` returned `2`.

**That is the complete list — there is no arithmetic, comparison, or boolean operator.** The DSL is deliberately minimal: `for_loop` covers bounded iteration, `{{var}}` covers string building, and `js_eval` is the escape hatch for real computation. If a step needs arithmetic, do it inside one `js_eval` and bind the result.

`js_eval` requires a browser, since it evaluates in page context.

---

## Variable substitution

Inside a `literal` or a `tool_call` input, strings are substituted two ways:

- A string that is **exactly** `$name` is replaced by the variable's value, keeping its type.
- Each `{{name}}` inside a longer string is replaced by the value as text (strings as-is, anything else as JSON). Spaces inside the braces are allowed: `{{ name }}`.

With `v = "WORLD"` and `n = 7`:

| Input | Output |
|---|---|
| `"$v"` | `"WORLD"` |
| `"hello_$v"` | `"hello_$v"` |
| `"hello_{{v}}"` | `"hello_WORLD"` |
| `{"embedded": "page={{n}}"}` | `{"embedded": "page=7"}` |
| `{"nested": ["$v", "x$v"]}` | `{"nested": ["WORLD", "x$v"]}` |
| `{"whole": "$n"}` | `{"whole": 7}` |

Things worth knowing:

1. **`$name` is type-preserving.** `"$n"` becomes the number `7`, not `"7"`. This is what lets you pass a captured object straight into a tool's `input`.
2. **Substitution recurses** through arrays and objects.
3. **`{{name}}` with an unknown variable fails the step** with `variable not found: name`. Braces around anything other than an identifier (`{{a: 1}}`) are left alone, so JS source in an `execute_js` input passes through.
4. **`js_eval` expressions get no substitution** — `$page_num` arrives in the browser as literal `$page_num`. To feed a variable to JS, run `execute_js` as a `tool_call` and put `{{var}}` in its `script` input.

---

## Static validation

The parser validates before anything executes. Referencing a variable that isn't in scope is a **parse error**, not a runtime error:

```
Error: script parse/validation failed: undefined variable `does_not_exist` in collect expression
```

Unknown tool names and empty `steps` are rejected the same way, so a typo like `read-contents` fails immediately rather than halfway through a crawl.

**`save_script` does not run these checks.** It calls only the JSON/schema parser, not the validator, so a script with undefined variables or unknown tools **saves successfully** and fails later at `run_script`. Treat a successful save as a syntax check only — `run_script` is where semantic validation happens.

---

## The 17 scriptable tools

Scripts may only call these:

```
navigate    click       click_at      fill_form    page_map
read_content  screenshot  go_back     scroll       wait
select_option  execute_js  hover      press_key    switch_tab
list_resources  save_file
```

**All 12 DevTools tools are excluded**, plus `refresh` and `set_device`. There is no way to script a network, console, WebSocket, performance, cookie, storage, coverage, or accessibility audit, and no way to script interception.

Structure around it: use a script to gather and navigate, then run the observability tools manually against the resulting session — or drive the whole audit with manual calls.

---

## Limits

Override them per run via `run_script`'s `limits`; omitted keys keep their defaults, so `{"max_steps": 50}` is enough. Each value is **capped** by the matching `settings.json` `script` entry (which defaults to the same numbers), so raising a limit above its default takes a settings change, e.g. `"script": {"per_step_timeout_secs": 90}`.

| Limit | Default | Meaning |
|---|---|---|
| `max_steps` | `200` | `tool_call` nodes only |
| `max_timeout_secs` | `300` | wall clock, whole script |
| `per_step_timeout_secs` | `30` | single tool call |
| `max_output_bytes` | `10485760` (10 MB) | collected + yielded combined |
| `max_parallel_branches` | `10` | branches in one `parallel` |
| `max_script_size_bytes` | `1048576` (1 MB) | script JSON size |
| `max_nesting_depth` | `10` | control-flow nesting |
| `max_concurrent_scripts` | `5` | settings-only; not overridable per run |

A per-step timeout is catchable by `try_catch`; step-limit and wall-clock terminations are not. In a `parallel` block the output byte budget is shared across branches.

---

## Concurrency is unsafe over MCP

**Run browser scripts one at a time.** Both concurrency mechanisms are currently broken, and one of them fails silently.

### `parallel` branches fail teardown with 2+ branches

Branch bodies execute, then the script ends `Failed` with `extracted_data` empty:

```
error: "tool error: failed to close parallel pages: page 3:
        Browser bridge protocol error: close_page_failed: Invalid page index 3"
```

Reproduced consistently:

| Branches | Browser | Result |
|---|---|---|
| 1 | yes | `Completed`, `extracted_data: ["Example Domain"]` |
| 2 | yes | `Failed` — `Invalid page index 3` (both navigations ran, `steps_executed: 2`) |
| 2 | no | `Failed` — `Invalid page index 2` |

The cleanup routine closes pages by index without accounting for indices shifting as earlier pages close. It also means `parallel` **requires a browser** even with no `tool_call` in its branches.

### Concurrent `run_script` calls corrupt data silently

`run_script` is non-blocking and `max_concurrent_scripts` defaults to 5, which makes firing several look attractive. **Do not.** The MCP server clones a single `BrowserContext` — a `Clone` type carrying `page_index` — and `spawn_script` never allocates a new page. Every concurrent script therefore drives the **same tab**.

Verified with two scripts navigating to pages with *different* titles. Script A loaded `example.com` and read `document.title`, but got script B's page — and reported success:

```json
[{"script_id":"scr_5ae589bf","status":"Completed","error":null,
  "extracted_data":[{"expected":"Example Domain",
                     "got":{"result":"All products | Books to Scrape - Sandbox"}}]},
 {"script_id":"scr_8599ebca","status":"Completed","error":null,
  "extracted_data":[{"expected":"All products | Books to Scrape - Sandbox",
                     "got":{"result":"All products | Books to Scrape - Sandbox"}}]}]
```

No error, no warning — just data from the wrong URL. A test using two pages with the *same* title cannot detect this, so verify isolation with distinguishable pages if you ever re-test it.

### What to do instead

- **Sequential scripts.** Run one, `wait_for_scripts`, then run the next. Slower, correct.
- **`run_goal` for genuine parallelism.** Its internal sub-agents each get their own tab, and a URL-claiming registry stops two children crawling the same page.
- **Batch inside one script.** One `execute_js` returning an array beats many round-trips, and `for_each` over enumerated URLs stays on one tab by design — which is safe precisely because it's sequential.

---

## Replaying a saved script without an agent

Once a script works, save it (`save_script`, or `run_script` with `save_as`) and hand the user a shell command instead of asking them to come back to you for every refresh. `acrawl run` needs a browser but no LLM or credentials, so it fits cron, CI, or any other program:

```bash
acrawl run mideast_headlines --format jsonl --flatten --output headlines.jsonl
```

`--format` is `json` (default, the `extracted_data` array), `jsonl` (one item per line), or `result` (the full result with `status` and `error`). It exits `0` on success, `1` on script failure (partial data still written), `2` if the script is missing or invalid, and `130` on Ctrl+C. Run `acrawl --help` for the limit flags.

---

## Verified examples

### 1. Substitution semantics (no browser)

Recorded before `{{var}}` interpolation existed; see [Variable substitution](#variable-substitution) for it.

Executor time ~1.5 ms since it contains no `tool_call` (the browser still initializes first over MCP).

```json
{
  "schema_version": 1,
  "name": "substitution_demo",
  "steps": [
    {"type": "assign", "variable": "v", "value": {"kind": "literal", "value": "WORLD"}},
    {"type": "assign", "variable": "n", "value": {"kind": "literal", "value": 7}},
    {"type": "collect", "value": {"kind": "literal", "value": "$v"}},
    {"type": "collect", "value": {"kind": "literal", "value": "hello_$v"}},
    {"type": "collect", "value": {"kind": "literal",
      "value": {"embedded": "page=$n", "nested": ["$v", "x$v"], "whole": "$n"}}}
  ]
}
```

Output:

```json
["WORLD", "hello_$v", {"embedded":"page=$n","nested":["WORLD","x$v"],"whole":7}]
```

### 2. Control flow and accessors (no browser)

```json
{
  "schema_version": 1,
  "name": "control_flow_demo",
  "steps": [
    {"type": "for_loop", "variable": "i",
      "from": {"kind": "literal", "value": 0},
      "to": {"kind": "literal", "value": 3},
      "steps": [{"type": "collect", "value": {"kind": "literal", "value": "$i"}}]},

    {"type": "assign", "variable": "arr", "value": {"kind": "literal", "value": ["a", "b"]}},
    {"type": "for_each", "variable": "it",
      "iterable": {"kind": "variable", "value": "arr"},
      "steps": [{"type": "collect", "value": {"kind": "literal", "value": "$it"}}]},

    {"type": "assign", "variable": "obj", "value": {"kind": "literal", "value": {"k1": 1, "k2": 2}}},
    {"type": "for_each", "variable": "o",
      "iterable": {"kind": "variable", "value": "obj"},
      "steps": [{"type": "collect", "value": {"kind": "literal", "value": "$o"}}]},

    {"type": "if_else", "condition": {"kind": "literal", "value": 0},
      "then_steps": [{"type": "collect", "value": {"kind": "literal", "value": "ZERO_IS_TRUTHY"}}],
      "else_steps": [{"type": "collect", "value": {"kind": "literal", "value": "ZERO_IS_FALSY"}}]},

    {"type": "collect", "value": {"kind": "array_index",
      "value": {"array": {"kind": "variable", "value": "arr"},
                "index": {"kind": "literal", "value": 1}}}},

    {"type": "collect", "value": {"kind": "field_access",
      "value": {"object": {"kind": "variable", "value": "obj"}, "field": "k2"}}}
  ]
}
```

Output:

```json
[0, 1, 2, "a", "b", {"key":"k1","value":1}, {"key":"k2","value":2}, "ZERO_IS_FALSY", "b", 2]
```

### 3. Navigate and capture a field

```json
{
  "schema_version": 1,
  "name": "title_grab",
  "steps": [
    {"type": "tool_call", "tool": "navigate",
      "input": {"url": "https://example.com", "content_depth": "none", "page_map_depth": "none"},
      "output": "p"},
    {"type": "collect", "value": {"kind": "field_access",
      "value": {"object": {"kind": "variable", "value": "p"}, "field": "title"}}}
  ]
}
```

Output: `["Example Domain"]`. Setting both depths to `none` keeps the navigation cheap when you only want one field.

### 4. Bulk extraction with `execute_js`

One `execute_js` returning an array beats N tool calls. This is the workhorse pattern for scraping a listing page — adapted from the shipped `multi_search` script.

```json
{
  "schema_version": 1,
  "name": "extract_links",
  "steps": [
    {"type": "tool_call", "tool": "navigate",
      "input": {"url": "https://example.com", "content_depth": "none", "page_map_depth": "none"}},
    {"type": "tool_call", "tool": "execute_js",
      "input": {"script": "Array.from(document.querySelectorAll('a[href]')).filter(a => a.href.startsWith('http') && a.innerText.trim().length > 3).slice(0, 20).map(a => ({ title: a.innerText.trim().split('\\n')[0], url: a.href }))"},
      "output": "links"},
    {"type": "collect", "value": {"kind": "variable", "value": "links"}}
  ]
}
```

Escape newlines as `\\n` inside the JSON string. The `execute_js` result object holds the value under `result`, so use `field_access` on `result` if you bind the raw output and need to unwrap it.

### 5. Resilient click with `try_catch`

```json
{
  "schema_version": 1,
  "name": "resilient_click",
  "steps": [
    {"type": "tool_call", "tool": "navigate", "input": {"url": "https://example.com"}},
    {"type": "try_catch", "error_var": "e",
      "try_steps": [
        {"type": "tool_call", "tool": "click", "input": {"selector": "#submit-btn"}}
      ],
      "catch_steps": [
        {"type": "collect", "value": {"kind": "literal", "value": "primary selector failed"}},
        {"type": "tool_call", "tool": "click", "input": {"text": "Submit", "role": "button"}}
      ],
      "finally_steps": [
        {"type": "tool_call", "tool": "wait", "input": {"seconds": 1, "silent": true}}
      ]}
  ]
}
```

Falling back from a CSS selector to a label+role match is the durable pattern, because label text survives class-name churn.

### 6. Paginated crawl with progress

Here the URLs are enumerated literals. To build them from a loop variable instead, iterate the page numbers and use `"url": "https://books.toscrape.com/catalogue/page-{{n}}.html"`.

```json
{
  "schema_version": 1,
  "name": "paginate",
  "steps": [
    {"type": "assign", "variable": "urls", "value": {"kind": "literal", "value": [
      "https://books.toscrape.com/catalogue/page-1.html",
      "https://books.toscrape.com/catalogue/page-2.html",
      "https://books.toscrape.com/catalogue/page-3.html"
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

`"url": "$u"` works because the whole string is the variable. The `yield` makes progress visible through `script_status` mid-run and preserves it across a cancel.
