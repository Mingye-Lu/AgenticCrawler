# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
