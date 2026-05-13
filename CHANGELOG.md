# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Removed

- Classic line-mode REPL. The bare `acrawl` command now always launches the Ratatui TUI; running it without a TTY on stdout exits with an error pointing at `acrawl prompt` (one-shot) and `acrawl --resume` (session maintenance).
- `classic_repl` field in `~/.acrawl/settings.json`. Existing files with this field set are ignored silently — no migration needed.

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

[0.2.2]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.2.2
[0.2.1]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.2.1
[0.2.0]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.2.0
[0.1.1]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.1.1
[0.1.0]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.1.0
