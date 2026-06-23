# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **reCAPTCHA v3 presence detection** (`recaptcha_detected`): `navigate` now reports `recaptcha_detected: true` in its JSON response when the page loads Google reCAPTCHA v3 (invisible, score-based) CDN scripts without a visible v2 widget. This is informational — presence alone does not indicate a block.
- **Silent form-submission warning** (`submission_warning`): when `fill_form(submit: true)` produces no visible page change (same URL, `changed: false`) and reCAPTCHA v3 is present, the reply includes a `submission_warning` field with a hedged explanation and the remedy (`acrawl config set headless false` / `--headed` / `/extension`).
- **Agent remedy guidance**: the system prompt now encodes a two-condition rule — `recaptcha_detected: true` is not a blocker; only a silent-submission (no page change after submit) should prompt the agent to report the headless remedy.

## [0.12.2] - 2026-06-23

### Added

- **`page_map` — semantic region tree and active dialog detection**: emits a `regions` tree with ephemeral `@rN` handles and human-readable labels (`sidebar`, `main panel`, `admin modal`); `active_dialog` points to the topmost visible overlay; non-`<form>` controls (div-based modal inputs) surface with accessible names. `scope` now also accepts semantic tokens (`"dialog"`, `"main"`, `"sidebar"`) and `@rN` handles in addition to raw CSS selectors.
- **`fill_form` — page-wide label resolution**: field keys resolve by visible label text page-wide without a `<form>` boundary, enabling fill in modal/admin UIs built from divs. Falls back to fuzzy matching via `crate::semantic::match_text`.
- **`select_option` — portal-aware custom dropdown engine**: handles ARIA combobox/listbox and div-based dropdowns whose option list renders in a portal outside the trigger's DOM subtree. Detects open state via four signals (aria-expanded flip, new listbox/menu, new options, new floating panel), locates the option container via aria-controls/owns then document-wide search, and selects keyboard-first with a click fallback. Omitting `value`/`label`/`index` opens and enumerates available options without selecting (list-options mode).
- **`click` — text + role + region activation**: new `text` parameter activates elements by visible label, including `<label for=id>` and wrapping `<label>` resolution. Optional `role` filter (supports semantic ARIA roles for native elements) and `region` filter (`@rN` handle or `"dialog"`/`"main"`/`"sidebar"` token). `selector` and `text` are mutually exclusive.

### Changed

- Post-action diffs auto-scope to the interacted container or active dialog by default; pass `widen: true` to any interaction tool to restore the full-page diff.
- Diff computation consolidated to Rust (`feedback.rs`); the Chrome extension's JS-side `computePageMapDiff` / `pageMapCache` removed — both backends flow through a single diff path.
- `@rN` region handles now resolve from the last full-page `page_map` snapshot rather than the most-recently-stored snapshot, so handles remain stable after container-scoped interactions.

### Fixed

- **click**: `@rN` region handle not in snapshot now hard-fails instead of silently widening scope to full page
- **click**: remove `|| document` JS fallback; scoped IIFE throws if the scope element is missing from the DOM
- **click, fill_form**: `aria-labelledby` now correctly resolved as a whitespace-separated id list
- **fill_form**: `submit` no longer reports success when no `<form>` matched the selector (`form_not_found` outcome now surfaces as an error)
- **select_option**: popup closure alone no longer counts as verified selection; only trigger-text change or `aria-selected` qualify
- **browser/playwright**: `isVisible` in DOM-snapshot extraction now uses `getBoundingClientRect()` matching the extension backend, fixing false-invisible on `position:fixed` portal overlays
- **feedback**: post-action snapshots now enriched before caching, so `@rN` region handles remain valid after interactions; bare-tag diff-scope selectors (`section`, `article`, `form`) replaced with id-based or role-scoped selectors to prevent anchoring to the wrong container

## [0.12.1] - 2026-06-21

### Added

- **Non-interactive CLI for agent/CI use** — all credential and configuration management is now scriptable without a TTY, enabling headless CI pipelines and agent-driven setup.
  - `acrawl auth <provider> --api-key <key>` (and provider-specific flags: `--access-key`/`--secret-key`/`--region` for Bedrock, `--resource-name`/`--deployment-name` for Azure, `--base-url` for custom endpoints) writes credentials non-interactively; `--json` emits a machine-readable result.
  - `acrawl auth status [--check <provider>] [--json]` reports configured providers with masked secrets; `--check` exits 0 if the provider is ready or 3 if not configured — suitable as a CI gate.
  - `acrawl auth list [--json]` lists all 25 supported providers with their env-var names.
  - `acrawl config get [key] [--effective] [--all] [--json]` and `acrawl config set <key> <value>` / `acrawl config unset <key>` read and write `settings.json` using dot-notation keys (e.g. `optimization.html_diff_mode`).
  - `acrawl mcp install --client <id,...> | --all [--scope user|project] [--yes] [--json]` and `acrawl mcp uninstall` equivalents run without prompts when non-interactive flags are supplied; `--list-clients --json` enumerates the 17 supported IDE clients.
- **Exit-code taxonomy** — all subcommands now follow a consistent contract: `0` = success, `1` = runtime error, `2` = usage / configuration error, `3` = provider not configured.
- **REPL TTY guard** — `acrawl` (bare invocation) now checks both `stdin` and `stdout` for TTY before launching the interactive REPL; non-TTY environments receive exit code `2` and a message pointing at `acrawl prompt` and `acrawl --resume`.

## [0.12.0] - 2026-06-21

### Added

- **Browser observation & DevTools toolset** — 13 new tools grow the toolbox from 29 to 42, giving the agent DevTools-style visibility into live page activity. All work across both the CloakBrowser and Chrome-extension backends and are built on a temporal observation system: every action bumps a monotonic `seq`, and the observation tools accept `since`/`until` windows so they can query "since the last action" (`BrowserBackend` gains `poll_observations` / `set_seq`).
  - **Network** — `list_network_activity` (requests with status, type, size, and duration; filtering and adjective-based sorting) and `inspect_request` (per-request detail: request/response headers, request/response bodies gated on `include_body`, and a per-phase timing breakdown — DNS, connect, TLS, TTFB, download).
  - **Console** — `list_page_logs` (console messages and uncaught exceptions/rejections, grouped by message/source/level, reporting verbatim console levels) and `inspect_log` (individual occurrences with stack traces).
  - **WebSocket** — `list_websocket_activity` and `inspect_websocket` (sent/received frames).
  - **Performance** — `get_page_performance` (Navigation/Resource Timing metrics plus the top resources by transfer size).
  - **Coverage** — `measure_coverage` (JS/CSS used-vs-total bytes, persisted across navigations).
  - **Storage** — `inspect_cookies` (security analysis with RFC 6265 third-party detection) and `inspect_storage` (local/session storage).
  - **Accessibility** — `audit_accessibility` (axe-core WCAG audit: wcag2a / wcag2aa / wcag21aa / wcag22aa, using cumulative tag sets).
  - **Network interception** — `intercept_network` (block or mock requests by glob or regex pattern).
  - **Navigation** — `refresh` (reload the current page).

## [0.11.1] - 2026-06-19

### Added

- **CDN block page detection** — the smart-fetch router now recognizes CDN/security block pages served over HTTP (Cloudflare challenges, Akamai access-denied pages, "you have been blocked by network security" interstitials, captcha gates) and auto-escalates them to the headless browser. A three-tier heuristic keeps false positives low: CSS-dominated walled-garden responses (>60% `<style>` content with <300 chars of visible text, e.g. Reddit's 32 KB CSS-variable dump), strong CDN HTML signatures (`__cf_chl_`, `cf-challenge`, Akamai markers), and a text-pattern-plus-structural-sparseness check gated behind an HTML-document shell so tag-less JSON/text error bodies are never escalated. HTML entities are normalized before matching, and normalization only allocates when an entity is actually present so the per-fetch hot path stays cheap. The escalation heuristics are also skipped entirely when no browser is available to escalate to.

## [0.11.0] - 2026-06-17

### Added

- **Device Emulation** (`set_device` tool) — switch between mobile and desktop browser emulation mid-session. 10 built-in presets (iphone_15, iphone_se, iphone_15_pro_max, pixel_7, galaxy_s24, ipad_pro, ipad, galaxy_tab_s9, desktop, desktop_hd) or custom viewport/UA/touch/scale parameters. Cookies and localStorage are preserved across switches. Returns a differential `page_state` showing responsive layout changes (collapsed navs, hidden elements, breakpoint shifts) rather than the full page_map. Cannot be used while sub-agents are running.

## [0.10.1] - 2026-06-17

### Fixed

- **Empty SPA shell detection** — replaced the naive "large HTML + sparse text" heuristic with a multi-signal scoring function that accumulates confidence from framework asset paths (`/_next/static/`, `/_nuxt/`, `ng-version=`), empty mount-point divs, noscript "enable JavaScript" messages, and bundler hash patterns. Pages with embedded data blobs (`__NEXT_DATA__`, `window.__NUXT__`, `data-reactroot`) now correctly skip browser escalation since their content is already server-rendered. Eliminates false positives on legitimate sparse pages (login forms, image-heavy landing pages).
- **SPA hydration wait** — after browser navigation, polls for visible text content (up to 3s in 300ms intervals) before capturing, so async-rendered SPAs like Gitee search have time to hydrate.

## [0.10.0] - 2026-06-11

### Added

- **HTML Diff Mode** (`optimization.html_diff_mode`) — on repeated visits to the same URL, only changed content sections are returned with `[unchanged: N sections]` markers, reducing token usage 50–70% on multi-turn sessions. Also active in MCP direct-tool mode (the server now maintains a persistent `CrawlState` across calls).
- **Action Loop Detection** (`optimization.loop_detection`) — rolling-window action hash detects repeated identical actions with escalating nudges (soft at 5, medium at 8, strong at 12 repeats); page stagnation detection after 5 consecutive identical page fingerprints.
- **Page Fingerprinting** (`optimization.page_fingerprinting`) — lightweight FNV-1a fingerprint (url + element_count + first-1000-char text hash) stored in CrawlState; used by loop detection and action caching for cache invalidation.
- **Planning Interval** (`optimization.planning_interval`) — every N steps injects planning-checkpoint or execution-mode guidance into the dynamic prompt; disabled by default (interval=0).
- **Failure Classification** (`optimization.failure_classification`) — 16-category keyword-based error taxonomy (zero LLM cost); `classify()` maps error messages to SelectorNotFound, CaptchaDetected, RateLimited, etc.; `retry_strategy()` returns RetryWithHealing, RetryWithDelay, NoRetry, or ResetAndRetry per category.
- **Self-Healing Selectors** (`optimization.self_healing`) — on SelectorNotFound/SelectorAmbiguous, fetches a fresh page_map and text-matches to the correct element ref; logs `[healed: @eOLD → @eNEW]`; zero LLM calls; max retries configurable (default 2).
- **Action Caching** (`optimization.action_caching`) — in-memory SHA-256 keyed cache for read-only tools (`page_map`, `read_content`, `list_resources`); invalidated on page fingerprint change; TTL-based expiry (default 30s). `execute_js` is intentionally excluded as it may have side effects.
- **Confidence Tracking** (`optimization.confidence_tracking`) — parses `[confidence: HIGH/MEDIUM/LOW]` from assistant responses; 2+ consecutive LOWs triggers stagnation alert via DynamicPromptContext; advisory only, never blocks.
- **Compound Component Enrichment** (`optimization.compound_enrichment`) — extends interactive element JSON with an `enrichment` field for complex form controls: date format hints, range min/max/step/value, number bounds, select option lists (max 20 + overflow count), file accept types, textarea maxlength. Max 200 bytes/element.
- **Content-Aware Cleaning Profiles** (`optimization.content_aware_profiles`) — `CleaningProfile` enum (Default/Minimal/Aggressive/ReadingMode) auto-selected by task keyword and content size; `select_profile()` picks ReadingMode for extraction tasks, Minimal for interaction tasks, Aggressive for content > 50KB.
- **Budget Enforcement** (`optimization.budget_max_session_cost_usd`, `optimization.budget_enforcement`) — `BudgetEnforcer` with Warn/Block modes; Warn injects budget warning into the dynamic prompt at configurable threshold (default 80%); Block terminates the agent loop cleanly when the cost limit is reached.
- **Per-Agent Cost Attribution** (`optimization.per_agent_cost_tracking`) — `build_cost_breakdown()` walks flat child sessions and reconstructs per-child cost via UsageTracker; `/cost` command shows per-agent breakdown when flag is ON.
- **Dynamic System Prompt Infrastructure** — `DynamicPromptContext` struct with four optional fields (stagnation_alert, planning_guidance, budget_warning, loop_nudge); injected as section 9 of the system prompt via a shared `Arc<Mutex<>>` slot; all optimizations write to this slot, runtime picks up on the next iteration.
- **Optimization Settings Schema** — nested `OptimizationSettings` struct in `Settings` with 18 fields, all `Option<T>` and defaulting to OFF for backward compatibility; 18 `settings_get_*` getter functions.

## [0.9.1] - 2026-06-10

### Changed

- **`navigate` defaults to `fit_markdown`** — the `format` parameter now defaults to `fit_markdown` instead of `markdown`, saving 30–40% tokens on typical pages. Pass `format: "markdown"` explicitly to restore full output.
- **`wait` returns `page_state`** — both the selector-based and fixed-duration wait branches now return a `page_state` diff (URL, title, added/removed/modified elements) after the condition resolves, consistent with all other action tools (`click`, `fill_form`, `press_key`, `scroll`, etc.). Eliminates the extra `page_map` call previously needed to observe what changed.

### Fixed

- **Script tools missing from `ToolRegistry`** — `run_script`, `wait_for_scripts`, `script_status`, `cancel_script`, `save_script`, `list_scripts`, and `read_script` were parsed and validated correctly but not dispatched by the agent loop. All 7 script tools are now registered.

### Improved

- **MCP tool descriptions** — all 28 tool descriptions and parameter schemas enriched with concrete examples, edge-case guidance, and clearer return-value documentation for better LLM tool selection and Glama TDQS scoring.

## [0.9.0] - 2026-06-09

### Added

- **Autonomous Script Protocol** — a new deterministic execution layer that lets the LLM run multi-step browser automation without per-step LLM round-trips. Write scripts once, execute them in tight loops — dramatically faster and cheaper for repetitive page patterns (pagination, form filling, bulk extraction).

- **New `crates/script/` crate** — standalone grammar, parser, and persistence layer:
  - AST types: `ScriptDefinition`, `ScriptNode` (10 node kinds), `Expression` (5 expression kinds)
  - `parse_script` + `validate_script` with comprehensive error reporting (unknown tools, undefined variables, excessive nesting, oversized scripts)
  - Disk persistence: `save_script_to_disk`, `load_script_from_disk`, `list_scripts_on_disk`

- **7 new script tools** (available in the agent loop, MCP server, and via `run_goal`):
  - `run_script` — execute an inline script or load a saved one by `name`; returns `script_id` immediately (non-blocking). Accepts `save_as` to persist the script after execution and `limits` to override defaults.
  - `wait_for_scripts` — block until script(s) complete and collect full `ScriptResult` (extracted_data, yielded_data, steps_executed, elapsed_secs, error)
  - `script_status` — non-blocking poll returning live state (step, items_collected, current_url, elapsed_secs, errors_caught)
  - `cancel_script` — abort a running script via cooperative cancellation token
  - `save_script` — persist a script definition to `~/.acrawl/scripts/<name>.json`
  - `list_scripts` — list all saved scripts with ISO 8601 UTC timestamps (`modified_at`) and file sizes
  - `read_script` — read back a full script definition from disk

- **Script execution engine** (`crates/agent/src/script_executor/`):
  - Supported nodes: `tool_call`, `assign`, `collect`, `yield`, `for_loop`, `for_each`, `while_loop`, `if_else`, `try_catch` (with `catch`/`finally`/`error_var`), `parallel`
  - Limits enforced at runtime: `max_steps`, `max_timeout_secs`, `per_step_timeout_secs`, `max_output_bytes`, `max_parallel_branches`, `max_nesting_depth`
  - `parallel` branches each get their own browser page; share a global step counter and cancellation token; `errors_caught` and `output_bytes` are propagated back to the parent executor on completion
  - `collect` accumulates to `extracted_data`; `yield` writes to a shared `Arc<RwLock<Vec<Value>>>` readable via `script_status` mid-execution
  - Variable substitution: `$varname` strings in tool inputs are replaced with their current values

### Fixed

- **`Expression` serde deserialization** — changed from internally-tagged (`#[serde(tag="kind")]`) to adjacently-tagged (`#[serde(tag="kind", content="value")]`). The old tag caused `Literal`, `Variable`, and `JsEval` newtype variants to fail deserialization from JSON — meaning no user-submitted script with variables or literals would parse.
- **MCP server `run_script` panic** — `spawn_script` calls `tokio::task::spawn` internally but was invoked outside any `block_on` context, causing an immediate `"no reactor running"` panic that killed the server process. Wrapped in `rt.block_on(async { … })`.
- **`cleanup_completed` result race** — `spawn_script` called `cleanup_completed()` before checking concurrency limits, silently evicting just-completed scripts from the map. `wait_for_scripts` would then return `NotFound` for fast-completing scripts. Removed the premature cleanup.
- **`max_output_bytes` not enforced** — the limit was stored and validated but never checked during execution. `push_extracted` and `push_yielded` now track accumulated byte count and return `ScriptExecutionError` on overflow.
- **`validate_script_name` duplicated with inconsistent rules** — three separate implementations in `save_script.rs`, `read_script.rs`, and `persistence.rs`. Consolidated into `persistence::validate_script_name` with the strictest ruleset (rejects leading dash, dots, path separators, non-normal path components).
- **`list_scripts` timestamp format** — `modified_at` previously returned a raw Unix epoch integer (e.g. `"1780991949"`). Now returns ISO 8601 UTC (e.g. `"2026-06-09T13:39:09Z"`) via `time::OffsetDateTime + Rfc3339`.

## [0.8.7] - 2026-06-08

### Added

- **`page_map_depth` parameter for navigate** — new `page_map_depth` option (`full`, `slim`, `none`, default: `slim`) controls how much structural data is returned inline with navigation responses. `slim` strips CSS selectors from links/headings/landmarks and caps link text at 60 chars, reducing token usage while preserving `@eN` refs for interaction. `none` omits the page_map entirely. Full page_map is still cached internally for differential feedback.
- **MCP server unit tests** — 18 tests covering `parse_run_goal_request`, `validate_tool_names`, `normalize_tool_name`, `filtered_tool_specs`, `execute_run_goal`, and framed/line-delimited protocol detection.
- **Render crate unit tests** — tests for `MarkdownStreamState` push/flush, incremental streaming, partial content boundaries, and long-line handling.

### Changed

- **`mvp_tool_specs()` refactored** — the 376-line monolithic tool specification function is now split into `navigation_tools()`, `interaction_tools()`, `extraction_tools()`, and `agent_control_tools()` helpers. Public API unchanged.

### Fixed

- **CloakBrowser-dependent tests skip gracefully** — tests requiring PlaywrightBridge now detect `PlaywrightNotInstalled` and return early instead of panicking, eliminating 3 false failures on machines without Node.js/CloakBrowser.

## [0.8.6] - 2026-06-08

### Added

- **`fit_markdown` format for navigate** — new `format="fit_markdown"` option that prunes boilerplate DOM nodes (ads, navs, sidebars, footers) before markdown conversion, dramatically reducing token consumption on noisy pages. Scores elements by text density, descendant link density, semantic tag weight, and class/id signals. Falls back to plain text when pruning removes all content. Tool instructions now recommend `fit_markdown` as the preferred default format.

## [0.8.5] - 2026-06-07

### Added

- **Stable `@eN` element references** — `page_map` now assigns short, stable handles (`@e1`, `@e2`, …) to each interactive element. Interaction tools (`click`, `hover`, `fill_form`, `press_key`, `select_option`) accept `@eN` in their selector fields, resolving them to the underlying CSS selector. This eliminates the need to copy long, fragile CSS paths — the LLM can just say `click @e3`.
- **RefMap data structure** (`crates/browser/src/ref_map.rs`) — maps integer IDs to CSS selectors with stable reuse (same selector always gets the same ref) and lifecycle management (clear on navigation).
- **Ref resolution module** (`crates/agent/src/tools/ref_resolve.rs`) — centralized `@eN` → CSS selector resolution shared across all interaction tools. Plain CSS selectors pass through unchanged for full backward compatibility.
- **Navigate embedded refs** — the `page_map` returned inline with `navigate` responses now includes `@eN` annotations, so the first page view the LLM sees already has stable handles (no extra `page_map` call needed).
- **Scoped `page_map` refs** — `page_map` with a `scope` parameter (e.g. modals/dialogs) now also annotates interactive elements with refs.
- **`NopBridge` test utility** (`crates/browser/src/testing.rs`) — no-op `BrowserBackend` implementation for unit testing `BrowserContext` without launching a real browser.
- **Glama MCP registry verification** — added `glama.json` for Glama marketplace discovery.

### Fixed

- **Ref invalidation on navigation** — `navigate`, `go_back`, and `switch_tab` now clear the ref map immediately, preventing stale refs from resolving against a different page and clicking wrong elements.
- **Bridge script launch on Windows** — the CloakBrowser bridge script is now written to `~/.acrawl/bridge.cjs` and executed via `node <path>` instead of `node -e <script>`, fixing the Windows command-line length limit (OS error 206) that prevented all browser features from working.
- **URL normalization deduplication** — consolidated duplicate URL-normalization helpers into a single shared `normalize_url` function used by both `page_map` and `feedback`.

## [0.8.4] - 2026-06-04

### Added

- **Differential page_map feedback** — interaction tools (click, fill_form, select_option, hover, press_key) now return a differential page state showing exactly what changed instead of a full page dump. Includes added/removed headings, links, landmarks, and interactive elements, plus state changes (disabled, checked, value, aria-expanded, aria-pressed, aria-selected). Falls back to full page_map when changes exceed the previous element count.
- **Interactive element value tracking** — page_map now captures the current `value` of select, input, and textarea elements (truncated to 60 chars). For selects, reports the selected option's display text.
- **Smithery MCP marketplace listing** — added `smithery.yaml` for Smithery discovery.
- **Dockerfile for MCP server introspection** — enables Glama verification and container-based deployment.
- **Smithery MCPB publish step** — release workflow now publishes to Smithery marketplace.

### Fixed

- **Navigate seeds page snapshot cache** — the first interaction after `navigate` now produces a differential response instead of falling back to a full page_map.
- **Hash-route fragment preservation** — cache keys now preserve `#/path` and `#!/path` fragments (hash-routed SPAs) while still stripping simple in-page anchors like `#section`.
- **Multiset-aware structural diff** — duplicate headings/links/landmarks are now correctly counted (previously collapsed by set-based comparison).

## [0.8.3] - 2026-06-04

### Added

- **MCPB bundles in releases** — each release now includes platform-specific `.mcpb` archives (ZIP of manifest.json + binary) for single-click installation in Claude Desktop and other MCP hosts. Five bundles: linux-x64, linux-arm64, macos-x64, macos-arm64, windows-x64.
- **Automated MCP Registry publishing** — the release workflow now automatically publishes acrawl to `registry.modelcontextprotocol.io` via GitHub OIDC after each release, making it discoverable in the MCP ecosystem.

## [0.8.2] - 2026-06-03

### Added

- **`page_map` interactive elements** — the `interactive` section now returns up to 30 actual elements with `text`, `selector`, `tag`, `type`, and ARIA state (`aria-pressed`, `aria-expanded`, `aria-selected`, `disabled`, `checked`, `role`). Covers buttons, inputs, selects, textareas, and ARIA widgets (role=button/tab/menuitem/option/switch/checkbox). Flat count keys (`buttons`, `inputs`, `selects`, `textareas`) preserved at root level for backward compatibility.
- **`page_map` scope parameter** — optional `scope` CSS selector restricts all queries to a container element (e.g. `scope: "[role='dialog']"` for modal-only content). Returns `scope_not_found: true` with empty sections if the selector doesn't match.
- **`wait` state parameter** — optional `state` field accepts `visible`, `hidden`, `attached`, or `detached`. Enables waiting for elements to become visible (not just exist in DOM) or disappear (e.g. loading spinners). Errors if `state` is provided without a `selector`.

### Changed

- `BrowserBackend::page_map()` trait method now accepts `scope: Option<&str>`.
- `BrowserBackend::wait_for_selector()` trait method now accepts `state: Option<&str>`.
- Extension backend visibility checks use `getComputedStyle` + `getBoundingClientRect` to match Playwright's stricter semantics.

## [0.8.1] - 2026-06-03

### Added

- **`click_at` tool** — new tool (#21) that dispatches real mouse clicks at specific viewport coordinates via Playwright's `page.mouse.click(x, y)`. Enables interaction with canvas elements, maps, SVGs, and UI components that lack stable CSS selectors. Schema is OpenAI strict-mode compatible (all properties required, no nullable optionals). Both CloakBrowser and Chrome extension backends supported.
- **`screenshot` element & format options** — the screenshot tool now accepts:
  - `selector` — screenshot a specific element (auto-scrolls into view, crops to element bounds)
  - `format` — `png`, `jpeg`, or `webp` output (JPEG/WebP produce 5-10x smaller files)
  - `quality` — compression level 0-100 for lossy formats
  - `full_page` — capture the entire scrollable page, not just the viewport
  - Saved filenames now use the correct extension (`.jpg`, `.webp`, `.png`) based on format
  - MCP server returns the correct `media_type` (`image/jpeg`, `image/webp`) instead of hardcoded `image/png`

### Changed

- Tool count is now 21 (17 browser + 4 agent-control). MCP server exposes 18 tools (17 browser + `run_goal`).
- `BrowserBackend::screenshot()` trait method now accepts a `ScreenshotOptions` struct instead of no arguments.

## [0.8.0] - 2026-06-03

### Added

- **MCP installer: 17 supported clients** — expanded `acrawl mcp install` to support 12 additional IDE/agent clients: OpenCode, Zed, TRAE, JetBrains IDEs, Gemini CLI, Qwen Code, Codex CLI, Hermes, OpenClaw, Goose, Crush, and Aider. Interactive installer auto-detects installed IDEs and writes per-client config.

### Removed

- **`wait_for_human` tool** — the human-in-the-loop pause tool and all supporting infrastructure (pause/resume state machine, `ChildLifecycle::Paused` variant, `RuntimeObserver` pause hooks, `ToolEffect::Pause`, TUI escalation UI) have been removed. Tool count is now 20. The agent no longer has a mechanism to pause and request user intervention.

### Fixed

- **Stale `paused` state in `subagent_status`** — the LLM-facing tool instructions incorrectly listed "paused" as a valid child state after its removal.
- **Dead code on Linux** — removed an unused `appdata_dir()` stub that was gated behind `#[cfg(not(windows))]` but never called (only used inside `#[cfg(windows)]` blocks).

## [0.7.6] - 2026-06-01

### Added

- **`acrawl-ui` crate** — shared CLI/TUI modules (display_width, error, output_sink, session_mgr, auth, app, events) extracted into `crates/ui`. Both `cli` and `tui` depend on it instead of sharing code via `#[path]` includes. Removes the `tui-crate-context` feature flag.
- **30 new tests** — session persistence (corrupt JSON, missing files, large history roundtrip), slash commands (all 17 commands, args, case insensitivity, resume-safe set), extension bridge (disconnected fast-fail, timeout, navigation), MCP server (empty/oversized goal rejection, missing fields).

### Fixed

- **TUI phantom "interrupted" on first tool call** — during streaming, the assistant message (containing a ToolUse block) was pushed to the transcript before tool results arrived, causing the historical renderer to show "◼ interrupted" while the live spinner also showed the same tool as running. Now skips rendering unresolved ToolUse blocks from historical messages when a turn is in progress.
- **Streaming reasoning content ordering** — reasoning blocks now close before text begins (previously stayed open until stream finish), and the TUI renders actual thinking text instead of the raw JSON wrapper.
- **Security hardening** — Windows ACL on `credentials.json`, 0600 permissions on `bridge.json`, block Windows Alternate Data Streams in `save_file`, replace manual URL parsing with `url::Url::parse()`, add `Zeroize` on `OAuthTokenSet`, add 100K character limit on MCP `run_goal` input.
- **CLI requires explicit subcommand** — `acrawl <words>` no longer silently treats bare words as a prompt. Produces a clear error with usage hint; use `acrawl prompt "..."` or `-p "..."`.
- **Duplicate help hint** — removed duplicate "try --help" message on CLI error output.
- **Build date** — replaced hardcoded `DEFAULT_DATE` with CI-injected `BUILD_DATE` env var. Dev builds show "unknown".
- **`install-browser` on Windows** — `npx` (a `.cmd` file) was not found by `std::process::Command`. Now routes through `cmd /C npx` matching the existing self-update fix.

### Removed

- **`ModelInfo.aliases`** — dead field removed from provider catalog (68 occurrences). Model format is strictly `provider/model-id` everywhere.

### Documentation

- Removed stale model alias references from README and AGENTS.md.
- Rewrote post-install welcome messaging (install.sh, install.ps1).
- Added browser modes section explaining CloakBrowser vs extension backends.

## [0.7.5] - 2026-06-01

### Added

- **Form field `id` in page_map** — `page_map` now includes the `id` attribute for form fields, making it easier for agents to target inputs by ID.

### Fixed

- **SPA content readiness** — `navigate` now waits for network idle (up to 3s) after `domcontentloaded`, ensuring SPA frameworks finish fetching data and rendering before page content and `page_map` are captured. Previously, SPA pages returned only nav/footer content.
- **HTTP response encoding** — enabled gzip/deflate/brotli decompression in the HTTP fetcher. Added charset-aware decoding: responses with explicit `Content-Type: charset=X` are transcoded via `encoding_rs`. Responses without a Content-Type header that appear to be GBK-encoded (detected via script heuristic) are decoded as GBK instead of producing mojibake.
- **`fill_form` selector resolution** — fields are now resolved by label text, case-insensitive ID, placeholder, and `aria-label` in addition to CSS selectors. Relaxed CSS detection heuristic to allow text containing dots and spaces.
- **`fill_form` SPA submit** — clicking the submit button (or dispatching a submit event) instead of calling `form.submit()`, which bypasses SPA framework handlers.
- **Post-action page state timing** — `click` and `fill_form` now detect URL changes (polling every 50ms for up to 2s) before capturing `page_state`, replacing a fixed sleep. This handles async SPA navigation that completes after the DOM action returns.
- **Empty SPA content fallback** — `navigate` falls back to visible page text when the main content extraction (`<main>`/`<article>`) yields empty for JavaScript-rendered pages.

## [0.7.4] - 2026-05-29

### Changed

- **God modules decomposed into focused submodules** — six large single-file modules (1.5k–5.7k lines each) have been split into directory modules with logical subfiles. Affected modules: `browser::playwright` (bridge, script, backend, types), `cli::app` (session, slash, turn), `runtime::compact` (summarize, transform), `runtime::conversation`, `tui::modals::auth` (draw, handlers), and `tui::repl_app` (event_loop, input_editor, layout, oauth_spawn, slash_commands, types). Public API and behavior are unchanged.

### Fixed

- **TUI prompt width regression** — the refactoring inadvertently altered the prompt width calculation; restored to the correct value.
- **Compact truncation test assertion** — a test assertion was tightened to match the actual compaction output.

## [0.7.3] - 2026-05-29

### Changed

- **TUI renders directly from `ConversationMessage`** — the transcript view no longer converts messages to an intermediate `TranscriptEntry` representation. The parent view now renders assistant text, tool calls, and user messages directly from the persisted message model, improving consistency between what is displayed and what is saved.
- **Session schema v2 with `child_sessions`** — sessions now persist sub-agent (child) tabs alongside the main conversation. On session load, child tabs are restored with their full transcript history.
- **`/clear` simplified** — removed the `--confirm` flag. Clearing now also resets child tab state and uses lazy session persistence (empty sessions are not written to disk).

### Added

- **Child tab persistence and restoration** — forked sub-agent sessions are captured on completion and stored in the session file. Switching sessions or resuming restores child tabs with their original transcript.
- **Tool result pairing** — a `build_tool_result_index` utility maps `tool_use_id` to tool outcomes, enabling correct historical rendering of completed tool calls (previously displayed as "interrupted" after turn end).

### Fixed

- **Child tab view no longer renders blank** — `draw_child_view` previously passed empty data to the renderer; it now renders from the child tab's actual transcript entries.
- **Tool calls no longer show as "interrupted" after turn completes** — tool-result messages are now propagated to TUI state, allowing the result index to pair them with their corresponding tool-use blocks.
- **`/clear` no longer leaves stale child tabs** — clearing a session resets the child tab panel and returns to the parent view.
- **Session schema migration** — v1 sessions are normalized to v2 on load, preventing ambiguous hybrid files on re-save.
- **`--resume /clear` no longer writes empty session files** — the resumed file is deleted instead of being overwritten with an empty session, matching the lazy-persistence design.

## [0.7.2] - 2026-05-28

### Fixed

- **`acrawl update` now updates CloakBrowser** — previously `install_cloakbrowser_if_needed()` only ran when the acrawl binary itself had a new version. If the binary was already current, the function was unreachable and CloakBrowser would sit on a stale version indefinitely. The update now always checks CloakBrowser regardless of binary version.
- **CloakBrowser update no longer fails silently** — added Node.js version detection (requires 20+), operation timeouts (2min npm, 5min browser download), stderr capture with tail output on failure, and actionable remediation messages. Also fixed npm/npx not being found on Windows (tokio `Command` doesn't resolve `.cmd` files).
- **TUI no longer corrupted by child process stderr** — PlaywrightBridge and MCP stdio processes used `Stdio::inherit()` for stderr, causing Node.js warnings, CloakBrowser update notices, and MCP server debug output to write directly into the terminal and corrupt the Ratatui display. Child stderr is now redirected to `~/.acrawl/stderr.log` when the TUI is active.

## [0.7.1] - 2026-05-28

### Fixed

- **Output directory now globally stable** — `save_file` and `screenshot --save` previously resolved the default `output_dir` relative to the current working directory, producing unpredictable output locations (especially in MCP mode where CWD depends on the IDE). Relative paths in `settings.json` now resolve against `~/.acrawl/` (e.g. the default `"output"` becomes `~/.acrawl/output/`). Absolute paths are unaffected.
- **Removed dead `ACRAWL_OUTPUT_DIR` env var** — was set in `main.rs` on startup but never read by any tool.

### Added

- **`output_dir` parameter on `save_file` and `screenshot`** — callers (especially MCP clients) can now pass an explicit output directory per tool call. Relative paths resolve against CWD; absolute paths are used as-is. When omitted, the global default applies.

## [0.7.0] - 2026-05-28

### Changed

- **Architecture: 10-crate workspace** — the monolithic `crawler` crate has been decomposed into focused, single-responsibility crates. New crates extracted: `acrawl-core` (shared types/traits/errors), `browser` (PlaywrightBridge, ExtensionBridge, FetchRouter, BrowserContext, WsBridgeServer), `agent` (agent loop, 21 tools, sub-agent fork/join, CrawlState), `render` (markdown rendering, tool output formatting, OutputSink), `mcp-server` (built-in MCP server + IDE installer), and `acrawl-tui` (Ratatui terminal UI). The transitional `crawler` shim has been removed entirely.

- **Dependency graph corrected** — `api` and `browser` crates no longer depend on `runtime` (previously inverted). `ApiClient`/`ApiRequest` traits and `config_home_dir`/`OAuthConfig` moved to `acrawl-core`; OAuth module moved to `api`. All internal crates use direct imports instead of re-export shims.

## [0.6.4] - 2026-05-26

### Added

- **`screenshot` save to disk** — the `screenshot` tool accepts two new optional parameters: `save` (boolean, default `false`) and `filename` (string). When `save` is `true`, the captured PNG is decoded from base64 and written to the configured `output_dir`. If `filename` is omitted a timestamped default (`screenshot_<unix_ms>.png`) is used. Existing callers that pass no parameters continue to receive `screenshot_base64` in the response unchanged.

### Fixed

- **`acrawl update` fails on large binary downloads** — GitHub's Azure Blob CDN closes connections without sending the TLS `close_notify` alert after transmitting all data. `rustls` treated this as a fatal error, causing `error decoding response body` during self-update. The downloader now uses chunked streaming that accepts the connection-close when `Content-Length` bytes have already been received; SHA256 checksum verification still catches actual corruption.

## [0.6.3] - 2026-05-26

### Added

- **Paste masking** — large pastes (>150 bytes or >30 lines, whichever triggers first) are stored behind a dim italic placeholder pill instead of flooding the input area. Bracketed paste sequences and `Ctrl+V` both route through this path. Atomic cursor navigation skips over masks as a single unit; `Backspace`, `Delete`, and `Ctrl+W` delete the entire mask in one keystroke; clipboard cut expands the mask before yanking; and pressing Enter expands all masks before submitting the message.
- **`acrawl mcp uninstall`** — removes the MCP server configuration from any previously configured IDE (Claude Code, Cursor, Windsurf, VS Code/Copilot, OpenCode). Presents a styled confirmation prompt and degrades gracefully when stdin is not a TTY.

### Fixed

- **Install override behavior** — `acrawl mcp install` now correctly removes and re-adds an existing MCP entry when reinstalling, instead of silently appending a duplicate.
- **Stray Enter events from pasted newlines** — terminals that bypass bracketed paste and deliver content character-by-character no longer cause a premature submission when a pasted newline arrives; the paste-burst accumulator suppresses the Enter until the burst is flushed.
- **Paste-burst must not arm Enter suppression mid-paste** — flushing a burst while another burst is in progress no longer incorrectly activates the Enter-suppression gate, preventing legitimate Enter keystrokes from being swallowed after a paste.

### Performance

- **Slash overlay skipped when input has no leading `/`** — `refresh_slash_overlay` is a no-op on every keystroke that does not start with `/`, eliminating the scan on the hot path for normal typing.

## [0.6.2] - 2026-05-23

### Added

- **Styled auth and uninstall prompts** — `acrawl auth` and `acrawl uninstall` now use `dialoguer` Confirm/FuzzySelect widgets instead of raw stdin reads. Auth presents a searchable provider picker with dynamic category-column padding; uninstall shows a styled confirmation prompt that degrades gracefully when stdin is not a TTY (`interact_opt`).
- **Progress spinners in `acrawl update`** — long-running network fetch and `npm install` steps now show animated `indicatif` spinners so the terminal is not silently blocked during updates.

### Fixed

- **MCP server opened `about:blank` on IDE launch** — the browser (CloakBrowser/Playwright) was launched eagerly at MCP server startup, causing a blank browser window to appear whenever an IDE loaded the MCP configuration. The browser is now initialized on the first tool call; launch failures surface as a tool-level error response instead of a process exit.

## [0.6.1] - 2026-05-23

### Added

- **Input undo / redo** (`Ctrl+Z` / `Ctrl+Y`) — stack-based, capped at 100 entries. Every text mutation (insert, delete, paste, cut) pushes a snapshot; undo pops the previous state and redo walks forward. Stack is trimmed on new edits after an undo (MCMR-JIM).
- **Select-all and cut** (`Ctrl+A` / `Ctrl+X`) — select-all marks the entire buffer; cut copies the selection to the clipboard and removes it (MCMR-JIM).
- **Mouse click-to-position, drag-select, and copy** — clicking in the input area moves the cursor to the character under the pointer; dragging extends a selection with inverted highlight rendering; `Ctrl+C` / `Ctrl+Insert` / right-click copies the selected text to the system clipboard (MCMR-JIM).
- **Paste burst detection** — characters arriving within 30 ms are accumulated and flushed as a single insert instead of being processed one-by-one, preventing `Enter` at the end of a paste from submitting prematurely (MCMR-JIM).
- **Byte-cursor cache** (`byte_cursor` field) — maintains a byte-level cursor alongside the char-level cursor so `insert_input_char`, `insert_input_str`, `backspace`, `delete`, and `move_left/right` are all O(1) instead of O(n) (MCMR-JIM).
- **Visual-line-aware Up/Down navigation** — cursor up/down now respects soft-wrapped lines, keeping the preferred horizontal column across empty paragraphs and line-width changes (MCMR-JIM).

### Fixed

- **`byte_cursor` not reset on submit** — the `/exit` branch, the main Enter submit path, and the slash-command Tab-completion path all cleared or replaced `input.text` without zeroing `byte_cursor`. On the next keystroke `insert_input_char` would index into a fresh empty string using the stale offset, causing a panic or silent text corruption.
- **Paste burst Enter detection swallowed submissions** — an operator-precedence bug (`&&` / `||`) meant that a fast typist pressing Enter within 30 ms of the previous character had the submission silently treated as a literal newline even when no paste buffer was active. The guard now requires `paste_buffer.is_some()` before activating burst mode.
- **Undo history was unbounded** — `record_input_undo_snapshot` pushed snapshots without trimming. Typing a long message character-by-character accumulated one full-text clone per keystroke. History is now capped at 100 entries.
- **Up-arrow from last visual line jumped to line 0** — the loop that located the current visual line never `break`ed when the cursor was on the last line, leaving `cur_vis = 0`. Replaced with `partition_point` (O(log n)) which correctly identifies the last line.
- **Paste over a selection left duplicate content** — `flush_paste_buffer` inserted pasted text without first removing the selected range, unlike `insert_input_char` and `insert_input_str`. Pasting over a selection now replaces it.
- **O(n) `chars().nth()` in visual-line boundary checks** — four call sites called `chars().nth(boundary − 1)` to detect `\n` separators on every Up/Down keystroke and render pass. `visual_line_info` now embeds a `starts_paragraph` flag in its return type, eliminating all four O(n) scans.

## [0.6.0] - 2026-05-22

### Added

- **MCP server** (`acrawl mcp`) — built-in Model Context Protocol server over stdio. Replaces the old standalone `acrawl-mcp-server` binary; now a subcommand of the main `acrawl` binary following the `gopls mcp` / `docker mcp` / `nx mcp` convention.
- **16 direct browser tools via MCP** — `navigate`, `click`, `fill_form`, `page_map`, `read_content`, `screenshot`, `go_back`, `scroll`, `wait`, `select_option`, `execute_js`, `hover`, `press_key`, `switch_tab`, `list_resources`, `save_file` are callable directly by the MCP client. Each tool uses a persistent `BrowserContext` shared across the session.
- **`run_goal` MCP tool** — autonomous agent mode: the MCP client hands off a natural-language crawl goal and acrawl drives it with its own internal LLM loop, returning structured results (summary, extracted data, step count). Requires LLM credentials.
- **`acrawl mcp install`** — interactive IDE installer. Auto-detects installed IDEs (Claude Code, Cursor, Windsurf, VS Code/Copilot, OpenCode), presents a `dialoguer` checkbox picker (Space to toggle, Enter to confirm), and writes the correct MCP config for each. Supports global (user-level) and project-level config scopes. For Claude Code, delegates to `claude mcp add`; falls back to direct JSON merge for all other IDEs and as a fallback path.
- **MCP startup banner** — `acrawl mcp` prints a human-readable "ready" message to stderr so users know the server is waiting for JSON-RPC when launched manually.

### Fixed

- **Blank `run_goal` goal rejected** — whitespace-only goal strings now return a `-32602` JSON-RPC error immediately instead of launching a crawl that would immediately fail (MCMR-JIM).
- **`max_steps` range validated before clamping** — values above 200 were previously silently truncated by `.min(200)` before the range check, making it impossible to return an error for out-of-range inputs. The raw value is now validated first.
- **Graceful browser startup failure** — on `acrawl mcp`, a Playwright launch failure now prints a human-readable error and `hint: run acrawl install-browser` instead of a raw Rust panic backtrace.
- **Leading whitespace drained before transport detection** — pipes or proxies that flush whitespace bytes before the first MCP message byte no longer cause an `InvalidData` error. The reader now consumes whitespace-only fills in a loop before detecting framed vs line-delimited mode.
- **`claude mcp remove` failures surfaced on re-install** — when `acrawl mcp install` retries a Claude Code entry (remove + re-add), errors from the remove step are now logged instead of silently discarded, preventing silent duplicate entries.
- **Binary path canonicalization warning** — if `fs::canonicalize()` fails (dangling symlink, deleted binary, certain CI paths), the installer now warns that IDE configs will use the bare `acrawl` name and rely on PATH lookup.
- **`allowed_tools` propagated to forked child agents** — when `run_goal` restricted an agent with `allowed_tools` and the LLM called `fork`, the child agent had no tool restrictions: it could call any tool and its system prompt advertised all 21 tools regardless. Both are now fixed: `fork.rs` propagates `allowed_tools` to the child and `CrawlerAgent::run()` filters `mvp_tool_specs()` by `allowed_tools` before building the system prompt.

### Removed

- **`list_builtin_tools` MCP tool** — exposed tool names that LLM clients would then hallucinate calling, causing wasted turns with `-32601` errors.
- **`acrawl-mcp` workspace crate** — absorbed into `acrawl-cli` as the `mcp_server` and `mcp_install` modules. The release now ships one binary instead of two.

## [0.5.1] - 2026-05-21

### Removed

- **`acrawl init` subcommand** — initialized an IDE coding workspace; not relevant to a web crawler.
- **Sandbox system** (`sandbox.rs`) — code-execution isolation inherited from the IDE era.
- **CCR remote infrastructure** (`remote.rs`) — Claude Code Router session management, no longer used.
- **Bootstrap plan logic** (`bootstrap.rs`) — IDE session bootstrap plans.
- **`--permission-mode` CLI flag** — three-tier IDE permission model (`read-only` / `workspace-write` / `danger-full-access`). Use `--allowedTools` to restrict which tools the agent can invoke.
- **Per-workspace config loading** — config is now global-only (`~/.acrawl/`).
- **IDE-framed system prompt** — replaced with a crawler-focused identity and operating procedure.

### Changed

- **`WORKSPACE_DIR` env var renamed to `ACRAWL_OUTPUT_DIR`** — exported for child processes and MCP servers; aligns with the `output_dir` settings field and `ACRAWL_CONFIG_HOME` naming convention.
- **`/status` and `/config`** — output now shows crawler-relevant info only (visited URLs, crawl state) rather than IDE workspace details.
- **Compaction** — tracks crawled URLs instead of opened files.
- **`unwrap_ccr_proxy_url` → `unwrap_proxied_mcp_url`** — internal MCP helper renamed to drop the CCR branding.
- **README Permission Model section** — replaced with `--allowedTools` documentation.
- **AGENTS.md** — updated architecture docs to reflect removed systems and current slash-command set.

## [0.5.0] - 2026-05-21

### Added

- **Chrome Extension Bridge** — alternative browser backend that drives the user's real browser via Chrome DevTools Protocol. Connects through a local WebSocket server (`/extension` command) with token-based authentication. Supports all 16 browser tools through CDP, enabling automation with the user's logged-in sessions, cookies, and extensions.
- **`BrowserBackend` trait** — extracted from `PlaywrightBridge` to enable multiple browser backends. Both `PlaywrightBridge` (CloakBrowser) and `ExtensionBridge` implement it.
- **`/extension` slash command** — starts the WebSocket bridge server and displays the auth token for the Chrome extension.
- **`/cloakbrowser` slash command** — switches back to headless CloakBrowser mode.
- **Extension auto-reconnect** — when `browser_backend` is set to `"extension"` in settings, the bridge server auto-starts and waits for the extension to connect.
- **`browser_backend` and `extension_bridge_port` settings** — persist browser backend preference across sessions.
- **Chrome MV3 extension** (`extension/` directory) — service worker with CDP command handlers, options page, keepalive, exponential backoff reconnection.

### Changed

- **`PlaywrightBridgeError` renamed to `BridgeError`** — the error type is now backend-agnostic.
- **`WsBridgeServer` split into submodules** — `auth.rs`, `session.rs`, `http.rs` for better maintainability.

### Security

- **Token auth with constant-time comparison** — 256-bit hex token generated per server start, never exposed via `/health` endpoint.
- **Origin validation** — requires valid 32-char Chrome/Edge extension ID format.
- **Rate limiting** — 5 failed auth attempts per IP in 60s window triggers 429.
- **Fail-fast on disconnect** — `ExtensionBridge` immediately rejects commands when no client is connected.

## [0.4.9] - 2026-05-20

### Added

- **`cancel_subagent` tool** — parent agent can abortively cancel one or more named children by id. Each child's `ControlState` receives a cooperative cancel signal and its `JoinHandle` is aborted immediately; the URL scope is released so another sibling can claim it. Returns a JSON snapshot of cancelled and not-found ids.
- **`subagent_status` tool** — read-only poll of per-child progress snapshots. Returns step, max\_steps, last tool called, last text delta, lifecycle state, and seconds since the last event. Safe to call between any steps — does not join, cancel, or mutate children.
- **Atomic `UrlClaimRegistry`** — sibling sub-agents cannot crawl overlapping URLs. Before a child is spawned, the fork supervisor claims its scope (`single_page`, `url_list`, or `url_pattern`) under a mutex; the claim is released via a RAII `ClaimGuard` when the child exits, is cancelled, or setup fails. Conflict details (exact URL, pattern, or pattern-vs-exact match) are surfaced to the LLM so it can adjust scope.
- **Typed `CrawlTask`** — the `fork` tool now carries a structured, validated work packet instead of a free-form string. Includes `objective`, `scope`, and an optional `max_steps` override.

### Fixed

- **`wait_for_subagents` no longer aborts children on timeout** — previously called `handle.abort()` when the deadline elapsed. Now re-inserts the handle and reports the child as `still_running`; cancellation is an explicit action via `cancel_subagent`.
- **Fork child IDs are globally monotonic** — a per-agent atomic counter replaces `child_tasks.len() + 1`, which recycled IDs after `wait_for_subagents` drained the map and made downstream lookups ambiguous between generations.
- **`UrlList` intra-list duplicate URLs are deduplicated** — submitting the same URL twice in a `url_list` scope previously inflated the registry count and caused spurious conflicts; duplicates are now silently deduplicated before insertion.

### Changed

- **Fork child step budget default raised from 15 → 100** — the previous default was too tight for real-world multi-step crawls.
- **`subagent_status` visibility is direct-children only** — the snapshot registry Arc is inherited one level deep; grandchildren populate their own parent's registry, not the root's. Documented explicitly.

### Removed

- **`deadline_secs` and `success_criteria` removed from `fork` tool** — these fields were parsed and stored but never consumed. Advertising them in the schema invited the LLM to send values it believed had effect; they have been removed from the struct, parser, and JSON schema.

## [0.4.8] - 2026-05-20

### Added

- **`acrawl install-browser` subcommand** — installs CloakBrowser and downloads the browser binary independently of the main installer. Checks for Node.js 20+, runs `npm install cloakbrowser playwright-core`, installs Linux system dependencies via `playwright-core install-deps chromium`, and pre-downloads the stealth Chromium binary. Idempotent: skips the npm step if packages are already present.

### Fixed

- **Missing Linux system libraries crash Chromium on launch** — `acrawl install-browser` and `install.sh` now run `npx playwright-core install-deps chromium` (Linux only) after the npm install step, installing the OS packages (`libatk`, `libcups`, `libgbm`, `libasound`, etc.) that Chromium requires but minimal Linux installs omit. Without this, the browser binary would exit immediately with a missing `.so` error even though all npm packages installed successfully.

## [0.4.7] - 2026-05-16

A security, correctness, and resilience pass covering 22 review-flagged issues across the Rust crates, the install scripts, and CI, plus five review-follow-up fixes layered on top.

### Security

- **Credentials file is owner-only** — `~/.acrawl/credentials.json` is `chmod 0600` before the atomic rename so API keys and OAuth tokens are never world-readable on shared hosts.
- **`navigate` rejects non-http(s) URLs** — `file://`, `javascript:`, `data:`, etc. are refused at the tool boundary, closing a local-file-disclosure / SSRF primitive.
- **API key bytes are zeroed end-to-end** — the auth modal's input buffer, the credential store's in-memory copy after save, and the serialized JSON buffer inside `save_credentials_to_path` are all wiped via the `zeroize` crate.
- **Install scripts validate the GitHub release tag** — `install.sh` and `install.ps1` reject anything that isn't a recognisable semver string before it flows into download URLs.
- **Release workflow passes the tag literally to awk** — `awk -v ver=...` instead of regex interpolation so a tag containing metacharacters can't break or broaden the changelog extraction.

### Fixed

- **`Usage::total_tokens` includes cache fields** — was previously undercounting `cache_creation_input_tokens` and `cache_read_input_tokens` (and therefore reported cost) when prompt caching was on.
- **HTTP body cap (32 MiB)** — `fetcher` streams chunk-by-chunk and refuses oversize bodies via a new `FetchError::BodyTooLarge`; a lying `Content-Length` header can no longer OOM the host.
- **MCP frame cap (64 MiB)** — `mcp/process` rejects oversized `Content-Length` headers before allocating the receive buffer.
- **TUI render loop can't be starved** — `drain_events` is capped at 256 events per frame and the typewriter backlog flushes wholesale rather than growing the `VecDeque` unbounded.
- **`/clear` strict-parses its flag** — `/clear --comfirm` is now `Unknown` instead of silently wiping the session.
- **Ctrl+C is suppressed while a modal is open** — no longer cancels the in-flight agent or orphans an OAuth thread; the modal owns its own cancel (Esc / `cancel_tx`).
- **Model modal can't double-fire** — selecting a model and pressing Enter twice rapidly used to apply the change twice; outcome is now consumed via `take_outcome`.
- **Fork child IDs are monotonic** — using `child_tasks.len()+1` recycled IDs after `wait_for_subagents` drained the map, making downstream lookups ambiguous between generations.
- **MCP request id wraps to 1 instead of pinning** — `take_request_id` used `saturating_add`, which would have pinned every subsequent id at `u64::MAX` and broken JSON-RPC correlation.
- **SSE parser errors on invalid UTF-8** — previously substituted `U+FFFD` silently, letting malformed bytes succeed-but-wrong through downstream JSON parsing; Gemini stream caller updated.
- **Resume parser validates command names at arg-parse time** — `acrawl --resume session.json /not-a-command` (or any command not in the resume-safe set) is rejected up front with a specific error instead of failing deep in the dispatcher.
- **Auth flows reuse the process tokio runtime** — `anthropic` / `openai` / TUI model fetch no longer spin up a fresh `Runtime` per call (which leaked threads on retries).
- **Wait-for-OAuth-callback progress messages** — emits a remaining-time line every 30 s so a slow IdP doesn't look like a hang.
- **Poisoned `pending_title` mutex is recovered** — session save no longer silently drops the auto-generated title if the title thread panicked.
- **MCP error diagnostics** — replaced cryptic "server process missing after initialization" with actionable text pointing at the server's stderr.
- **Credential-load failures surface** — `load_credentials_or_warn` warns to stderr on a corrupt credentials file before defaulting, instead of silently presenting a fresh re-auth prompt.
- **Non-JSON tool input is logged** — when the model returns invalid JSON for a tool call, the parse error + tool name + use-id are logged before the input is wrapped in `{"raw": ...}`.
- **LLM-summarization fallback is no longer silent** — `eprintln` warnings on the API-error / empty-response / empty-after-compression branches so falling back to the mechanical summary stops being invisible. Oversized responses now flow through `compress_summary_text` (which already enforces the char budget) instead of being hard-rejected on a byte-length check.
- **Playwright Turnstile bypass no longer double-fetches** — the navigate handler was overwriting `bypassTurnstileIfPresent`'s return value with a redundant `page.content()` call, doubling the bridge round-trip per navigate.

### Changed

- **`ToolError` is now a typed enum** — `Message(String)` and `RequiresAsync { tool_name }`. The agent loop identifies async-only tools via `error.is_requires_async()` instead of substring-matching the error text, eliminating a misclassification path where any error message that happened to mention the canonical phrase could be mistaken for the sentinel. ~40 `ToolError(s)` tuple-struct call sites swept to `ToolError::new(s)`.
- **Summary line selection is O(N)** — `select_line_indexes` uses running char/line counters; the previous version recomputed the joined-char count per candidate, which was O(P · N · S) per priority pass.
- **`compact` continuation-prefix detection has a single source of truth** — `is_compact_continuation_message` in `compact.rs` replaces a literal-substring probe that lived in `conversation.rs`.

## [0.4.6] - 2026-05-16

### Added

- **`acrawl uninstall` subcommand** — removes the binary, `node_modules` (browser automation), and the config home directory. Pass `--purge` to also delete `settings.json`, `credentials.json`, and `sessions/`. Always prompts for confirmation. On Windows the running binary is renamed to `.old` (deleted if possible) and the User PATH entry is removed automatically via PowerShell.

### Fixed

- **Installer missing `playwright-core`** — both `install.ps1` and `install.sh` now install `playwright-core` explicitly alongside `cloakbrowser`. Previously, npm skipped it as an optional peer dependency, causing the CloakBrowser bridge to fail at runtime with `ERR_MODULE_NOT_FOUND`. Re-running the installer on a broken install auto-repairs it.

## [0.4.5] - 2026-05-16

### Added

- **Cloudflare Turnstile bypass** — after navigation, the Playwright bridge detects Turnstile challenge pages and performs human-like mouse sweeps to satisfy the behavioural model, polling up to 8 s for clearance.

### Fixed

- **Resume parser accepts command arguments** — `--resume session.json /clear --confirm` no longer rejects non-slash trailing arguments; args are grouped with their preceding slash command.
- **Preset provider auth routing** — selecting a preset provider (e.g. Groq, Mistral) in the TUI auth modal now correctly routes to the appropriate input step (placeholder URL editing, API key, or device-code OAuth) instead of falling through to the generic "Other" flow.
- **Auth modal Esc navigation** — pressing Esc from the API key input for a preset that required URL editing now returns to the base-URL step (preserving provider context) instead of jumping to provider selection.
- **Model modal hint** — added `(unconfigured → auth prompt)` hint to the model selector.
- **Reasoning effort feedback** — toggling reasoning effort (Ctrl+T) now prints a system message confirming the new level.

## [0.4.4] - 2026-05-15

### Added

- **Tool output pruning** — `compact_session()` now walks backward through messages before summarizing and truncates `ToolResult` outputs that fall outside a configurable protected window (`prune_protect_tokens`, default 40K tokens) to `prune_max_output_chars` (default 2K chars), appending a truncation marker. Dramatically reduces context consumed by large `navigate` results in long sessions.
- **Token-budget tail** — the compaction window is now determined by a backward token-budget walk (`preserve_recent_tokens`, default 80K tokens) instead of a fixed message count. `preserve_recent_messages` is retained as a hard minimum floor; a new `preserve_recent_messages_floor` field (default 2) ensures at least N messages are always preserved regardless of budget.
- **Summary merging across compactions** — on a second (or later) compaction, `merge_compact_summaries()` combines the existing compacted summary with the new one into **Previously compacted context** / **Newly compacted context** / **Key timeline** sections, preserving highlights from prior rounds instead of discarding them.
- **Summary prefix detection** — `extract_existing_compacted_summary()` detects an existing compaction prefix and `compact_session()` skips it when slicing the removed-messages window, preventing the summary from summarizing itself.
- **Priority-based summary compression** — new `summary_compression` module implements greedy line selection by priority tier (core detail lines → section headers → bullet items → other), with deduplication, whitespace normalization, per-line truncation, and an omission notice. Applied after merging to cap total summary size (`max_summary_chars`, default 1200 chars).
- **LLM-powered summarization** (opt-in) — when `compaction_llm_summarization = true` in `settings.json`, `maybe_auto_compact()` attempts to replace the mechanical summary with a structured LLM summary (Goal / Progress / Key Decisions / Next Steps / Relevant Files). Falls back silently to the mechanical summary on any failure.
- **Compaction settings** — six new fields in `settings.json`: `compaction_prune_protect_tokens`, `compaction_prune_max_output_chars`, `compaction_preserve_recent_tokens`, `compaction_preserve_recent_messages_floor`, `compaction_max_summary_chars`, `compaction_llm_summarization`. All are optional with backward-compatible defaults.

## [0.4.3] - 2026-05-14

### Changed

- **CloakBrowser binary downloaded during install** — `install.sh`, `install.ps1`, and `acrawl update` now explicitly run `npx cloakbrowser install` to download the browser binary up front, eliminating the cold-start delay on first `navigate` call. Falls back gracefully to lazy download if the explicit step fails.
- **`acrawl update` always refreshes CloakBrowser** — the update command now upgrades the cloakbrowser npm package to `@latest` (previously skipped if the package directory already existed), ensuring users on older versions get the current browser engine.

## [0.4.2] - 2026-05-14

### Changed

- **Markdown renderer rewritten** — replaced `tui-markdown` with a custom `pulldown-cmark` event consumer. Fixes nested list rendering (items no longer flatten to top-level), styled list content (bold/italic) no longer splits to the next line, and tables render with Unicode box-drawing borders.

### Fixed

- **Nested list indentation** — sub-bullets under numbered items now render with proper depth-based indentation instead of appearing as top-level items (`tui-markdown` issue #88).
- **Typewriter streaming boundary** — TUI typewriter now buffers to stream-safe block boundaries (closed fences, paragraph breaks) instead of flushing per-line, preventing mid-block rendering artifacts in code fences and tables.

## [0.4.1] - 2026-05-14

### Added

- **DeepSeek provider** — `deepseek/deepseek-chat` (V3, 128K context) and `deepseek/deepseek-reasoner` (R1, 64K context, reasoning) via the OpenAI-compatible ChatCompletions API. Set `DEEPSEEK_API_KEY` or configure via `acrawl auth`.

### Fixed

- **Model selection after auth** — providers other than Anthropic and OpenAI (Groq, Mistral, xAI, DeepSeek, etc.) now show the model picker after entering an API key. Previously they jumped straight to the success screen. The picker tries `models.dev` first and falls back to the built-in catalog.
- **DeepSeek-reasoner `reasoning_content` round-trip** — `reasoning_content` from `deepseek-reasoner` responses is now captured from the SSE stream and included in subsequent requests, fixing the `400 reasoning_content must be passed back` error on multi-turn conversations.
- **`/headed` on Linux without a display server** — `/headed` now checks `$DISPLAY` / `$WAYLAND_DISPLAY` before switching and shows a clear error if neither is set, instead of crashing Chromium silently.

## [0.4.0] - 2026-05-14

### Changed
- Replaced Playwright with CloakBrowser as the browser engine — passes bot detection (reCAPTCHA v3 0.9, Cloudflare Turnstile, FingerprintJS) with source-level stealth patches
- Human-like interaction enabled by default (Bézier mouse curves, natural keyboard timing)
- Browser binary auto-downloads on first use (no separate `npx playwright install` step)
- Minimum Node.js version raised from 16 to 20 (required by CloakBrowser)

## [0.3.4] - 2026-05-14

### Added

- **Child agent progress view** — full-screen view (`Ctrl+X`) renders child agent activity with the same styled transcript as the parent: tool calls show animated spinner/tick/cross, LLM text renders with markdown styling via `PredictiveMarkdownBuffer`, and line wrapping respects terminal width.
- **Child event pipeline** — `ChildEventKind` variants (TextDelta, ToolCallStart/Complete, StepStarted, PauseRequested, Resumed, Finished) convert to `TranscriptEntry` and render through the shared `build_wrapped_list` pipeline.
- **Child `wait_for_human` escalation** — when a child agent calls `wait_for_human`, the TUI auto-navigates to that child's view. Parent view shows a prominent yellow hint with the pause reason. Enter resumes the child via `ChildControlRegistry`.
- **Text selection in child view** — drag-to-select and right-click copy (OSC52) now works in child view, matching parent behavior.
- **Per-child scroll state** — each child tab has independent `ListState`-based scrolling with keyboard (j/k/G/g/PgUp/PgDn) and mouse wheel support.

### Changed

- Child tab data model migrated from `VecDeque<String>` to `Vec<TranscriptEntry>` + `ListState` + `PredictiveMarkdownBuffer` for rendering parity with parent.
- Bounded child transcript storage enforced at 1000 entries per child.
- On child `Finished`, any still-running tool entries are marked as interrupted.

### Known Issues

- Child panel view has unresolved rendering issues — `wait_for_human` auto-navigation and the parent hint indicator may not trigger reliably at runtime despite passing unit tests. Full interactive QA pending.

## [0.3.3] - 2026-05-13

### Added

- **`/sessions` slash command** — opens a scrollable TUI modal listing all saved sessions by title (with id fallback), sorted by last-modified. Up/Down/PgUp/PgDn/wheel to navigate, type to filter, Enter to switch, Esc to close. Inspired by OpenCode's `DialogSessionList`.
- **In-modal session management** — `Ctrl+X` deletes the highlighted session (two-press confirm with a red-highlighted row; deleting the active session starts a fresh one). `Ctrl+R` renames the session inline (current title prefilled; Enter saves, Esc cancels).
- **LLM-generated session titles** — after the first user message, a short title is generated in the background and saved to the session JSON. Naming runs in parallel with the main turn so it adds no perceived latency; the title appears in the `/sessions` picker.
- **Global session storage** — sessions now live under `ACRAWL_CONFIG_HOME` (default `~/.acrawl/sessions/`), matching the convention already used by `credentials.json` and `settings.json`. The same session list is visible from any working directory.

### Changed

- **Sessions are created lazily** — no JSON file is written on startup. The session file appears on disk only after the first user message, so quickly opening and closing `acrawl` no longer pollutes the sessions directory.
- `acrawl init` no longer adds `.acrawl/sessions/` to project `.gitignore` templates (sessions are no longer cwd-relative).

### Removed

- **`/resume` slash command** — replaced by the modal picker. The `--resume` startup flag is unchanged.
- **`/session` slash command** (with `list`/`switch` subcommands) — replaced by `/sessions` opening the modal. In non-TUI mode, `/sessions` prints a stub message since interactive picking isn't possible there.

## [0.3.2] - 2026-05-13

### Added

- **`content_depth` parameter on `navigate`** — controls how much page content is returned: `main` (default, extracts article/main content only), `full` (everything), `slim` (first 2000 chars), `none` (page_map only). Dramatically reduces context usage on content-heavy pages.
- **`strip_images` parameter on `navigate`** — defaults to `true`, removing markdown image syntax that wastes context on long CDN URLs the agent cannot render.
- **Post-action `page_state`** on all interaction tools — click, fill_form, scroll, hover, press_key, select_option, go_back, and switch_tab now return structural page context (headings, landmarks, links) after each action.
- Shared `htmd`-based markdown conversion module with safe fallback (strips tags instead of leaking raw HTML on conversion failure) and resilient code-fence wrapping.
- Expanded `page_map` — now returns headings, landmarks, forms (capped at 10), links (capped at 50), interactive element counts, and page metadata. Caps enforced inside the browser's `page.evaluate` to avoid large stdio transfers.

### Changed

- `navigate` default output format is now `main` content depth (was effectively `full`), reducing typical output by 60%+.
- `scroll` input parameter renamed from `amount` to `pixels` with unit description.
- `switch_tab` response no longer has top-level `url`/`title` (moved into `page_state`).
- `nav` and `footer` HTML tags are now always stripped during markdown conversion.

### Removed

- Dead filter parameters from `list_resources` tool schema.

## [0.3.1] - 2026-05-12

### Fixed

- `acrawl update` no longer short-circuits on the 24-hour startup-check cache. The explicit update command now always queries GitHub for the latest release; the TUI startup banner still uses the cache.

## [0.3.0] - 2026-05-12

### Removed

- Classic line-mode REPL. The bare `acrawl` command now always launches the Ratatui TUI; running it without a TTY on stdout exits with an error pointing at `acrawl prompt` (one-shot) and `acrawl --resume` (session maintenance).
- `classic_repl` field in `~/.acrawl/settings.json`. Existing files with this field set are ignored silently — no migration needed.

### Fixed

- `/exit` and `/quit` now interrupt any running task and exit immediately, even while busy.

## [0.2.2] - 2026-05-12

### Fixed

- Interrupts (Ctrl+C / double-Esc) are now preemptive — they abort mid-stream and mid-tool-execution immediately rather than waiting for the current operation to finish.
- Spinner switches to a static stop indicator (◼) during interruption with "Interrupting…" label in both the transcript overlay and footer.
- In-flight tool call entries in the transcript are marked as interrupted instead of spinning forever.
- Interrupted tool calls now insert proper `tool_result` stubs so the API does not reject the next turn with a missing-result 400 error.

## [0.2.1] - 2026-05-11

### Fixed

- Deprioritize screenshot tool in favor of direct text extraction.

### Added

- `acrawl update` self-update subcommand.

## [0.2.0] - 2026-05-08

### Added

- Human-in-the-loop: `wait_for_human` tool pauses the agent and auto-switches to headed browser so the user can solve CAPTCHAs or log in manually. Press Enter to resume.
- Pause hotkey (`P`) during busy state pauses the agent between iterations.
- Cookie export/import in PlaywrightBridge for session persistence across navigations.
- Update check on startup with 24-hour cache; shows "update available" card on welcome screen.
- Cross-platform install scripts (Linux/macOS shell, Windows PowerShell) with SHA256 verification.
- Tag-triggered CI release workflow building binaries for 5 targets (linux x64/arm64, macOS x64/arm64, Windows x64).
- NODE_PATH fallback for standalone Playwright resolution.
- Tri-state `ControlSignal` (continue/pause/cancel) replacing the simple AtomicBool cancel flag.

### Changed

- Removed `done` tool — the agent now finishes by emitting a final text response with no further tool calls.
- System prompt refactored into separate section functions.

### Fixed

- Lost-wakeup race in pause wait loops.
- localStorage restored on correct origin after navigation.
- NODE_PATH appended instead of overwritten for Playwright resolution.
- Browser tests serialized with env_lock to prevent flakiness.

## [0.1.1] - 2026-04-30

### Added

- Multi-line animated braille spinner for classic REPL output.

### Changed

- Consolidated tool formatting into a shared module used by both TUI and classic REPL.
- Removed deprecated `login`/`logout` subcommands (use `acrawl auth` instead).

### Fixed

- Tool detail lines now deferred correctly when pending tools remain in `StdoutSink`.

## [0.1.0] - 2026-04-29

### Added

- LLM-driven agent loop with a 19-tool toolbox (16 browser + 3 agent-control).
- 24 LLM providers: Anthropic, OpenAI, Google Gemini, AWS Bedrock, Azure OpenAI, Vertex AI, GitHub Copilot, SAP AI Core, GitLab Duo, Groq, Cerebras, DeepInfra, Together AI, Mistral, Perplexity, xAI, Cohere, Alibaba DashScope, OpenRouter, Vercel AI, Cloudflare Workers/Gateway, Venice AI, and any OpenAI-compatible endpoint.
- Smart fetch routing — HTTP-first with automatic escalation to headless Chromium when JS framework markers, auth redirects, or `<noscript>` bodies are detected.
- Sub-agent parallelism via `fork`/`wait_for_subagents`/`done` with configurable concurrency limits, fork depth, and step budgets.
- Interactive TUI REPL with markdown rendering, syntax highlighting, streaming output, model picker, and auth modal.
- 16 slash commands: `/help`, `/status`, `/model`, `/compact`, `/clear`, `/cost`, `/session`, `/export`, `/resume`, `/config`, `/auth`, `/headed`, `/headless`, `/debug`, `/version`, `/exit`.
- Session persistence with auto-save, resume, export to markdown, and auto-compaction.
- Permission model: `read-only`, `workspace-write`, `danger-full-access`.
- MCP (Model Context Protocol) server support with stdio, SSE, HTTP, and WebSocket transports.
- One-shot CLI mode via `acrawl prompt "..."`.
- Model aliases for quick switching (`sonnet`, `opus`, `haiku`, `4o`, `o3`, etc.) and provider-prefixed names.
- Reasoning effort cycling (`Ctrl+T`) for reasoning models.
- OAuth PKCE flow for Codex and GitHub Copilot, AWS SigV4 for Bedrock, GCP auth for Vertex AI.
- Structured output in JSON, CSV, or plain text.
- Credential management via `acrawl auth` with per-provider configuration.

[0.12.2]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.12.2
[0.12.1]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.12.1
[0.12.0]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.12.0
[0.11.1]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.11.1
[0.11.0]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.11.0
[0.10.1]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.10.1
[0.10.0]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.10.0
[0.9.1]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.9.1
[0.9.0]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.9.0
[0.8.7]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.8.7
[0.8.6]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.8.6
[0.8.5]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.8.5
[0.8.4]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.8.4
[0.8.3]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.8.3
[0.8.2]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.8.2
[0.8.1]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.8.1
[0.8.0]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.8.0
[0.7.6]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.7.6
[0.7.5]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.7.5
[0.7.4]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.7.4
[0.7.3]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.7.3
[0.7.2]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.7.2
[0.7.1]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.7.1
[0.7.0]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.7.0
[0.6.4]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.6.4
[0.6.3]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.6.3
[0.6.2]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.6.2
[0.6.1]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.6.1
[0.6.0]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.6.0
[0.5.1]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.5.1
[0.5.0]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.5.0
[0.4.9]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.4.9
[0.4.8]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.4.8
[0.4.7]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.4.7
[0.4.6]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.4.6
[0.4.5]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.4.5
[0.4.4]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.4.4
[0.4.3]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.4.3
[0.4.2]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.4.2
[0.4.1]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.4.1
[0.4.0]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.4.0
[0.3.4]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.3.4
[0.3.3]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.3.3
[0.3.2]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.3.2
[0.3.1]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.3.1
[0.3.0]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.3.0
[0.2.2]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.2.2
[0.2.1]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.2.1
[0.2.0]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.2.0
[0.1.1]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.1.1
[0.1.0]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.1.0
