# AGENTS.md

## Project

`acrawl` is a native-Rust LLM-driven web crawler. A user provides a natural-language goal; the agent plans, navigates, and extracts structured data via a 15-tool browser toolbox. It ships as a single binary with an interactive Ratatui REPL and non-interactive modes.

## Commands

```bash
cargo build --release                                        # produce ./target/release/acrawl
cargo test --workspace                                       # run full test suite (~316 tests)
cargo test -p <crate> <test_name>                            # run a single test (e.g. -p crawler mvp_tool_specs_contains_expected_15_tools)
cargo clippy --workspace --all-targets -- -D warnings        # lints must be clean (workspace lints set pedantic = warn)
cargo fmt --check                                            # format check

npx playwright install chromium                              # required for the Playwright bridge
./target/release/acrawl                                      # launch REPL
./target/release/acrawl prompt "scrape all titles from example.com"   # one-shot
./target/release/acrawl --resume session.json /status /compact        # non-interactive session maintenance
```

The CLI reads LLM credentials from `~/.acrawl/credentials.json` (managed by `acrawl auth`) and runtime settings from `~/.acrawl/settings.json`. Both paths respect the `ACRAWL_CONFIG_HOME` env var override. Run `acrawl auth [anthropic|openai|other]` to configure a provider.

## Workspace layout

Five crates under `crates/`, compiled with `resolver = "2"`:

- **acrawl-cli** — binary entry (`src/main.rs`), arg parsing, REPL (`src/tui/`), markdown/spinner rendering, session management, provider selection. `app.rs` owns `LiveCli` and the three provider code paths.
- **api** — HTTP + SSE clients for Anthropic (`client.rs`), OpenAI-compatible (`openai.rs`), and Codex OAuth (`codex.rs`). `sse.rs` is the shared streaming frame parser; `types.rs` holds the Anthropic message schema.
- **runtime** — `ConversationRuntime` (the core turn loop), `Session` persistence, system-prompt builder, permission model (`PermissionMode` = ReadOnly / WorkspaceWrite / DangerFullAccess), compaction, usage/pricing, OAuth PKCE, and a full MCP client stack (`mcp*.rs`).
- **commands** — slash-command registry (`/help`, `/status`, `/model`, `/compact`, `/clear`, `/cost`, `/session`, `/export`, `/resume`, `/config`, `/memory`, `/init`, `/diff`, `/version`). Knows which commands are safe to replay in `--resume`.
- **crawler** — the 15 browser tools, agent loop (`agent.rs`), `FetchRouter` that escalates HTTP→browser, and `PlaywrightBridge` — a child `node` process running an inline JS script (`PLAYWRIGHT_BRIDGE_NODE_SCRIPT` in `playwright.rs`) that speaks newline-delimited JSON over stdio.

## Architecture: how a turn actually flows

1. `acrawl-cli::app::LiveCli` builds a `ProviderClient` via `ProviderRegistry` from the persisted `CredentialStore` (`credentials.json`), plus a `ToolExecutor` backed by `crawler::ToolRegistry`.
2. `runtime::ConversationRuntime::run_turn` drives the loop: call `ApiClient::stream` → feed `AssistantEvent`s (text deltas, tool_use, usage, stop) → execute tools through `ToolExecutor` → append results → repeat until the model emits `MessageStop` with no tool calls or `MAX_STEPS` is hit.
3. The crawler tool handlers (`crates/crawler/src/tools/*.rs`) take JSON input, consult `CrawlState`, and act through a `BrowserContext` that wraps either the `FetchRouter` (reqwest HTTP path) or the `PlaywrightBridge` (headless Chromium). The router auto-escalates from HTTP to the browser when JS is needed.
4. `runtime::PermissionPolicy` gates every tool call against the current `PermissionMode`. Each of the 15 `ToolSpec`s declares `required_permission` — extraction/listing are `ReadOnly`, `save_file` is `WorkspaceWrite`, the rest require `DangerFullAccess`.
5. `runtime::UsageTracker` + `pricing_for_model` feed `/cost` and `/status`. `runtime::compact` watches `ACRAWL_AUTO_COMPACT_INPUT_TOKENS` (default 200k) and auto-compacts the session when the threshold trips.

The Playwright bridge is notable: it is a **single embedded Node script** launched as a subprocess, not a Rust Playwright binding. This is why `npx playwright install chromium` is a runtime requirement, not a build-time one.

## Provider routing

`ProviderRegistry` (in `crates/api/src/provider/mod.rs`) owns the model catalog and routes to the correct client:

- If `credentials.json` has an `active_provider`, that provider is used regardless of model name.
- Otherwise the registry infers the provider from the model id: models in the built-in catalog map to their declared `provider_id`; unknown models fall back to `"other"`.
- `resolve_alias` expands short names (`sonnet` → `claude-sonnet-4-6`, `4o` → `gpt-4o`, etc.) before routing.
- `build_client` constructs an `Anthropic`, `OpenAi`, or `Custom` (OpenAI-compatible chat/completions) client from the stored `StoredProviderConfig` for that provider.

Default model comes from the `default_model` field in the active provider's `StoredProviderConfig` inside `credentials.json`. `--model` on the CLI overrides it.

## Tool surface

`crawler::mvp_tool_specs()` returns the canonical 15-tool list with JSON schemas and required permission. `--allowedTools` accepts canonical names and the aliases `read`/`write`/`edit`/`glob`/`grep` → `read_file`/`write_file`/`edit_file`/`glob_search`/`grep_search`, but the **crawler toolset does not include those IDE tools** — attempting to allow `read_file` is an error and there's a regression test asserting this. When you add or rename a tool, update `mvp_tool_specs`, add a handler in `tools/mod.rs`, and adjust the count assertion in `crates/crawler/src/lib.rs` tests.

## Conventions specific to this repo

- `unsafe_code = "forbid"` at the workspace level — do not introduce `unsafe`.
- Clippy `pedantic` is on as a warning; `module_name_repetitions`, `missing_panics_doc`, `missing_errors_doc` are explicitly allowed. New lint warnings should be fixed rather than suppressed locally unless there's a reason.
- Tests that mutate process env (provider, model, workspace dir) must serialize with a `OnceLock<Mutex<()>>` guard, following the pattern in `acrawl-cli/src/main.rs` and `crates/runtime/src/lib.rs::test_env_lock`.
- Slash-command behavior is shared between the live REPL and `--resume`. When editing a slash command, check `resume_supported_slash_commands()` — the test `resume_supported_command_list_matches_expected_surface` pins the exact resume-safe set.
- TUI popup/list UX baseline (applies to slash overlay + auth modal lists + similar list selectors):
  - Keep one blank line at the top of popup content.
  - Keep key-hint text pinned to the last visible content row, with a blank separator row above it and no extra blank row below it; style hints in dim gray.
  - Up/Down navigation must clamp at edges (no wrap-around) for both keyboard and mouse wheel.
  - For list selectors, Left jumps to the first item and Right jumps to the last item.
  - When scrolling to keep selection visible, use edge-follow behavior (no forced centering jumps).
- `claw-code/` at the repo root is a separate nested project (has its own `CLAUDE.md`/`README.md`). It is not part of the `acrawl` cargo workspace — don't pull it into workspace-wide edits unless the task is specifically about it.
