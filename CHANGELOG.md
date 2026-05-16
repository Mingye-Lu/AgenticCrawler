# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
