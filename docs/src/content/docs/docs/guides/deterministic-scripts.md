---
title: "Repeatable crawls with zero LLM calls"
description: "Let the agent work out a crawl once, then replay it as a JSON script with loops and branches and zero LLM calls, from MCP or acrawl run."
---

**Short answer:** let the agent work out a crawl once, then run the repetitive part as a JSON script. Scripts support loops, conditionals and parallel branches, and they call acrawl's tools directly, with no LLM call per step.

A ReAct-style agent calls the model on every page. If a job pages through 50 search results, that's at least 50 model calls for work that is identical each time. A script does the same 50 pages for zero.

## A minimal script

This script visits two catalog pages and collects every book's title and price:

```json
{
  "schema_version": 1,
  "name": "books_catalog",
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

`schema_version` is required and must be `1`, and `steps` can't be empty.

## Running and managing scripts

| Tool | What it does |
|---|---|
| `run_script` | Starts a script and returns a `script_id` right away |
| `wait_for_scripts` | Blocks until scripts finish and returns their results |
| `script_status` | Checks progress without blocking |
| `cancel_script` | Stops a running script |
| `save_script` / `list_scripts` / `read_script` | Store scripts on disk and reuse them across sessions |

`yield` after each page makes partial progress visible while a long script runs.

## Replaying a saved script from the shell

A saved script needs a browser but no LLM, so `acrawl run` replays it from a terminal, cron job or CI step without waking an agent or configuring a provider:

```bash
acrawl run books_catalog                                   # saved name → JSON array on stdout
acrawl run ./scripts/books.json --format jsonl --flatten -o books.jsonl
acrawl run books_catalog --format result                   # full result: status, error, yielded data
```

| Option | What it does |
|---|---|
| `--format json\|jsonl\|result` | `json` (default) prints the collected data as one array, `jsonl` prints one item per line, `result` prints the full result envelope |
| `--output`, `-o PATH` | Writes to a file instead of stdout, atomically, so readers never see a partial file |
| `--flatten` | Unwraps one level of arrays, so one `collect` per page becomes one record per item |
| `--max-steps N`, `--timeout SECS`, `--step-timeout SECS` | Per-run limits, capped by the `script` section of `settings.json` |
| `--quiet`, `-q` | Hides progress (`yield` values) and the summary on stderr |

Exit codes: `0` completed, `1` the script failed or the browser couldn't start, `2` unknown or invalid script (checked before the browser launches), `130` interrupted with Ctrl+C. Partial data is still written on `1` and `130`.

`acrawl run` uses the same browser backend as everything else, so in extension mode it replays the script in your own logged-in browser. If the extension doesn't connect, it exits with an error rather than falling back to the headless browser.

## Tips

- Run one script at a time. Scripts share the session's tab, so concurrent runs can read each other's pages. For real parallelism, use `run_goal`, whose sub-agents each get their own tab.
- Mix the two: let the agent handle the unpredictable steps (search, login, deciding what to compare) and script the repetitive ones.

The full grammar, including conditionals and parallel branches, is in [the scripting reference](https://github.com/Mingye-Lu/AgenticCrawler/blob/main/skills/acrawl-mcp/references/scripting.md).
