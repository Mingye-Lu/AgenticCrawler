---
title: "Configuration"
description: "Every acrawl setting: where the config files live, the defaults in settings.json, script limits, compaction, and the optional token and cost optimizations."
head:
  - tag: title
    content: "acrawl configuration: settings.json, credentials and optimizations"
  - tag: meta
    attrs:
      property: og:title
      content: "acrawl configuration: settings.json, credentials and optimizations"
---

**Short answer:** acrawl keeps its configuration in two JSON files under `~/.acrawl/`. `credentials.json` holds provider keys and is managed by `acrawl auth`. `settings.json` holds everything else and is managed by `acrawl config`. Every setting is optional, and a missing one uses its default.

## Where the files live

| Path | What it holds | Managed by |
|---|---|---|
| `~/.acrawl/credentials.json` | Provider keys, OAuth tokens, default model per provider | `acrawl auth` |
| `~/.acrawl/settings.json` | Browser, agent, script and optimization settings | `acrawl config` |
| `~/.acrawl/scripts/` | Saved [deterministic scripts](/AgenticCrawler/docs/guides/deterministic-scripts/) | `save_script`, `acrawl run` |
| `~/.acrawl/bridge.json` | Which process owns the [browser extension](/AgenticCrawler/docs/guides/real-browser/) bridge | acrawl, automatically |

Set `ACRAWL_CONFIG_HOME` to move the whole directory, for example to keep a separate profile per project or in CI.

## Read and change settings

```bash
acrawl config get                       # everything in settings.json
acrawl config get model --effective     # the value acrawl will actually use
acrawl config set headless false
acrawl config set optimization.html_diff_mode true   # dot notation for nested keys
acrawl config unset browser_backend
acrawl config path                      # print the settings file location
```

Inside the terminal UI, `/config` shows the same values. Command-line flags such as `--model` and `--headed` override the file for one run.

## `settings.json`

| Setting | Default | What it does |
|---|---|---|
| `model` | unset | Default model in `provider/model-id` form; falls back to the provider's `default_model` |
| `headless` | `true` | Run the bundled browser without a window |
| `max_steps` | `50` | Most tool calls the agent may make for one goal |
| `output_dir` | `"output"` | Where `save_file` writes |
| `reasoning_effort` | unset | `high`, `medium` or `low`, for reasoning models |
| `auto_compact_input_tokens` | `200000` | Input-token total that triggers automatic compaction |
| `max_concurrent_per_parent` | `5` | Sub-agents one agent may run at once |
| `max_fork_depth` | `3` | How deep sub-agents may nest |
| `max_total_agents` | `10` | Cap on sub-agents across the whole run |
| `fork_child_max_steps` | `100` | Step budget for each sub-agent |
| `fork_wait_timeout_secs` | `60` | How long `wait_for_subagents` waits |
| `browser_backend` | unset | `"extension"` drives your own browser; unset uses the bundled CloakBrowser |
| `extension_bridge_port` | `19876` | Local port for the extension bridge |
| `extension_bridge_connect_timeout_secs` | `3` | How long a browser tool waits for the extension to connect |

### Script limits (`script.*`)

These bound what a single deterministic script may do.

| Setting | Default | What it does |
|---|---|---|
| `script.max_steps` | `200` | Steps per run, counting loop iterations |
| `script.max_timeout_secs` | `300` | Wall-clock limit for a run |
| `script.per_step_timeout_secs` | `30` | Limit for one step |
| `script.max_parallel_branches` | `10` | Branches in one `parallel` step |
| `script.max_concurrent_scripts` | `5` | Scripts running at once |
| `script.max_nesting_depth` | `10` | How deeply steps may nest |
| `script.max_output_bytes` | `10485760` | Size of the collected data (10 MB) |
| `script.max_script_size_bytes` | `1048576` | Size of the script file (1 MB) |
| `script.scripts_dir` | `~/.acrawl/scripts` | Where saved scripts live |

`acrawl run` also accepts `--max-steps`, `--timeout` and `--step-timeout` to tighten these for one run.

### Compaction

Long sessions are compacted automatically once input tokens pass `auto_compact_input_tokens`. acrawl keeps the most recent 80,000 tokens of messages word for word, never splits a tool call from its result, trims old tool output to 2,000 characters, and replaces the rest with a summary of requests, URLs and pending work. Browser state is untouched: the open tab, cookies and page stay as they were.

The summary is built from a template, with no extra model call. Set `compaction_llm_summarization` to `true` to have the model write it instead. The thresholds are adjustable with `compaction_preserve_recent_tokens`, `compaction_preserve_recent_messages_floor`, `compaction_prune_protect_tokens`, `compaction_prune_max_output_chars` and `compaction_max_summary_chars`.

## Optimizations (`optimization.*`)

These change how the agent spends tokens and recovers from mistakes. Three are on by default because they cost nothing and only help; the rest are opt-in.

| Setting | Default | What it does |
|---|---|---|
| `failure_classification` | on | Sorts tool errors into 16 categories, such as `SelectorNotFound` or `RateLimited`, so the agent gets a specific recovery hint. No model calls. |
| `self_healing` | on | When a selector no longer matches, re-maps the page and finds the element by its text. Retries up to `self_healing_max_retries` (default 2). |
| `content_aware_profiles` | on | Picks how aggressively to clean a page from the task: reading mode for extraction, minimal for interaction, aggressive above 50 KB. |
| `html_diff_mode` | off | On a repeat visit to a URL, returns only the sections that changed. Cuts tokens by about half on multi-turn sessions. |
| `page_fingerprinting` | off | Lightweight page fingerprints, used by loop detection and action caching. |
| `loop_detection` | off | Spots the agent repeating itself and nudges it, more firmly each time. Tune with `loop_detection_window` (20) and `loop_nudge_threshold` (5). |
| `action_caching` | off | Caches read-only tool results until the page changes or `action_cache_ttl_secs` (30) passes. |
| `planning_interval` | `0` | Every N steps, asks the agent to stop and re-plan. `0` turns it off. |
| `confidence_tracking` | off | Has the agent rate its confidence after each step; two low ratings in a row trigger a re-think. |
| `compound_enrichment` | off | Adds date formats, ranges and option lists for complex form controls to the page map. |
| `semantic_prune` | off | Lets `navigate` take a `focus` and drops page blocks that a relevance model rates as off-topic. Sends page text to TypeSafe and needs a key in `semantic_prune_api_key` or `TYPESAFE_API_KEY`. |
| `budget_max_session_cost_usd` | unset | Spending limit for a session, in US dollars. |
| `budget_enforcement` | unset | `warn` tells the agent it is near the limit; `block` stops the session at the limit. |
| `budget_warn_threshold_pct` | `80` | Share of the budget at which warnings start. |
| `per_agent_cost_tracking` | off | Shows each sub-agent's cost in `/cost`. |

A cost-conscious profile for long sessions:

```json
{
  "optimization": {
    "html_diff_mode": true,
    "page_fingerprinting": true,
    "action_caching": true,
    "loop_detection": true,
    "budget_max_session_cost_usd": 0.5,
    "budget_enforcement": "warn"
  }
}
```

## `credentials.json`

You rarely edit this by hand; `acrawl auth` writes it. For reference, each provider entry may hold:

| Field | Meaning |
|---|---|
| `auth_method` | `api_key`, `oauth` or `aws_sigv4` |
| `api_key` | The provider key |
| `oauth` | Access and refresh tokens, expiry and scopes |
| `default_model` | Model used when no `--model` or `model` setting is given |
| `base_url` | Custom endpoint, for local servers or Azure |

The top-level `active_provider` decides which provider handles requests. Azure entries also carry `resource_name` and `deployment_name`; Bedrock entries carry `aws_access_key_id`, `aws_secret_access_key` and `region`; Vertex entries carry `gcp_project_id` and `gcp_region`.

## Related

- [LLM providers](/AgenticCrawler/docs/providers/)
- [Command-line reference](/AgenticCrawler/docs/cli/)
