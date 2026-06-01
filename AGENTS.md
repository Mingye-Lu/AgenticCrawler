# AGENTS.md

## Project

`acrawl` is a native-Rust LLM-driven web crawler. A user provides a natural-language goal; the agent plans, navigates, and extracts structured data via a 21-tool toolbox (18 browser + 2 agent-control + 1 human intervention). It ships as a single binary with three modes: an interactive Ratatui TUI REPL (requires a TTY), non-interactive `prompt` (one-shot) / `--resume` (slash-command replay), and `mcp` (built-in MCP server over stdio).

## Commands

```bash
cargo build --release                                        # produce ./target/release/acrawl
cargo test --workspace                                       # run full test suite (~770 tests)
cargo test -p <crate> <test_name>                            # run a single test (e.g. -p agent mvp_tool_specs_contains_expected_21_tools)
cargo clippy --workspace --all-targets -- -D warnings        # lints must be clean (workspace lints set pedantic = warn)
cargo fmt --check                                            # format check

./target/release/acrawl                                      # launch REPL
./target/release/acrawl prompt "scrape all titles from example.com"   # one-shot
./target/release/acrawl mcp                                  # launch MCP server (stdio)
./target/release/acrawl mcp install                          # interactive IDE installer
./target/release/acrawl --resume session.json /status /compact        # non-interactive session maintenance
```

The CLI reads LLM credentials from `~/.acrawl/credentials.json` (managed by `acrawl auth`) and runtime settings from `~/.acrawl/settings.json`. Both paths respect the `ACRAWL_CONFIG_HOME` env var override. Run `acrawl auth [anthropic|openai|other]` to configure a provider.

## Workspace layout

Ten crates under `crates/`, compiled with `resolver = "2"`:

- **core** (`acrawl-core`) — shared types, traits, and error hierarchy used across the workspace. Defines `ToolSpec`, `ToolEffect`, `AssistantEvent`, `RuntimeObserver`, `ContentBlock`/`ConversationMessage`/`MessageRole`/`TokenUsage`, `ToolOutcome`, `ApiClient`/`ApiRequest`, `config_home_dir`, and `OAuthConfig`.
- **api** — HTTP + SSE clients for Anthropic (`client.rs`), OpenAI-compatible (`openai.rs`), and Codex OAuth (`codex.rs`). `sse.rs` is the shared streaming frame parser; `types.rs` holds the Anthropic message schema. `oauth.rs` contains OAuth PKCE helpers, credential persistence, and token exchange types. `provider/registry.rs` and `provider/factory.rs` handle provider discovery and client construction.
- **browser** — browser automation layer. `PlaywrightBridge` (CloakBrowser headless Chromium), `ExtensionBridge` (Chrome extension backend via CDP), `FetchRouter` (HTTP→browser escalation), `BrowserContext` (tab/URL state), and `WsBridgeServer` (WebSocket server for extension communication). `browser_backend.rs` defines the `BrowserBackend` trait that both bridges implement.
- **agent** — agent orchestration and the 21-tool toolbox (18 browser + 2 agent-control + 1 human intervention). `agent.rs` drives the agent loop; `tools/` contains individual tool handlers; `manager.rs` manages sub-agent fork/join lifecycle; `prompt.rs` builds the system prompt; `state.rs` holds `CrawlState`; `url_claim.rs` coordinates URL claims across agents.
- **runtime** — `ConversationRuntime` (the core turn loop), `Session` persistence, system-prompt builder, compaction, usage/pricing, `config/` subdirectory (loader, MCP config, features), and a full MCP client stack in `mcp/` (`client.rs`, `types.rs`, `server_manager.rs`, `process.rs`, `naming.rs`).
- **render** — markdown/terminal rendering (`markdown.rs`), tool call output formatting (`tool_format.rs`), output format selection (`format.rs`), and the `OutputSink` trait + implementations (`sink.rs`) that bridge runtime events to the UI.
- **mcp-server** — built-in MCP server (`server.rs`: JSON-RPC over stdio, 16 direct browser tools + `run_goal`) and the interactive IDE installer (`installer.rs`: `acrawl mcp install`).
- **tui** (`acrawl-tui`) — Ratatui terminal UI. `repl_app.rs` owns the application state; `repl_render.rs` handles rendering; `events.rs` processes input; `modals/` contains auth, model-picker, and slash-command overlay widgets.
- **cli** — thin binary entry point (`main.rs`) + orchestration. `app/` directory owns `LiveCli` and provider code paths (`api_client.rs`, `tool_executor.rs`, `model_support.rs`, `runtime_builder.rs`, `resume.rs`). `session_mgr.rs` manages sessions; `output_sink.rs` bridges events to output; `self_update.rs` handles `acrawl update`.
- **commands** — slash-command registry (`/help`, `/status`, `/model`, `/compact`, `/clear`, `/cost`, `/session`, `/export`, `/resume`, `/config`, `/auth`, `/headed`, `/headless`, `/extension`, `/cloakbrowser`, `/debug`, `/version`, `/exit`). Knows which commands are safe to replay in `--resume`.

## Architecture: how a turn actually flows

1. `cli::app::LiveCli` builds a `ProviderClient` via `ProviderRegistry` from the persisted `CredentialStore` (`credentials.json`), plus a `ToolExecutor` backed by `agent::ToolRegistry`.
2. `runtime::ConversationRuntime::run_turn` drives the loop: call `ApiClient::stream` → feed `AssistantEvent`s (text deltas, tool_use, usage, stop) → execute tools through `ToolExecutor` → append results → repeat until the model emits `MessageStop` with no tool calls or `MAX_STEPS` is hit. The runtime notifies a `RuntimeObserver` at each event (text deltas, tool calls, turn end); `OutputSink` (`StdoutSink` for non-interactive `prompt`/`--resume`, `ChannelSink` for TUI) implements this trait to bridge events to the UI.
3. The crawler tool handlers (`crates/agent/src/tools/*.rs`) take JSON input, consult `CrawlState`, and act through a `BrowserContext` that wraps either the `FetchRouter` (reqwest HTTP path) or the `PlaywrightBridge` (headless Chromium). The router auto-escalates from HTTP to the browser when JS is needed.
4. The optional `--allowedTools` CLI flag restricts which tools are available; `CliToolExecutor` enforces this before execution. `ToolSpec` has no permission tier — all 21 tools are unrestricted by default.
5. `runtime::UsageTracker` + `pricing_for_model` feed `/cost` and `/status`. `runtime::compact` watches `ACRAWL_AUTO_COMPACT_INPUT_TOKENS` (default 200k) and auto-compacts the session when the threshold trips.

The CloakBrowser bridge is notable: it is a **single embedded Node script** launched as a subprocess, using CloakBrowser (not stock Playwright) for stealth browsing. The browser binary auto-downloads on first use — no separate install step needed.

## Extension bridge (Chrome extension backend)

An alternative to CloakBrowser: a Chrome MV3 extension that lets acrawl drive the user's real browser via CDP (Chrome DevTools Protocol). The system has three layers:

1. **`WsBridgeServer`** (`crates/browser/src/ws_server/`) — A tokio TCP server listening on `127.0.0.1:<port>` (default 19876). Handles `/health` (reachability check, no auth info) and `/bridge` (WebSocket upgrade with token auth + origin validation). Single-client gate: only one extension connection at a time.
2. **`ExtensionBridge`** (`crates/browser/src/extension.rs`) — Implements the `BrowserBackend` trait. Sends `{id, action, payload}` JSON commands over the WebSocket and awaits `{id, ok, result/error}` responses. Fails fast if no client is connected (checks `watch::Receiver<bool>`).
3. **Chrome Extension** (`extension/`) — MV3 service worker (`background.js`) that connects to the bridge server, dispatches CDP commands to Chrome tabs, and returns results. Command handlers live in `extension/commands/*.js`.

Key design decisions:
- `BrowserBackend` trait (`browser_backend.rs`) is the abstraction — both `PlaywrightBridge` and `ExtensionBridge` implement it. Error type is `BridgeError` (not backend-specific).
- Bridge server auto-starts only when `settings.browser_backend == "extension"`. Mode activation (`extension_mode`) is event-driven: it flips only when the extension actually connects, not when the server starts.
- Token auth uses a 256-bit hex token with constant-time comparison. Token is generated per-server-start and displayed via `/extension` command. The `/health` endpoint does NOT expose the token.
- Origin validation requires valid 32-char Chrome/Edge extension ID format.
- `/extension` starts the bridge server and shows the token. `/cloakbrowser` tears down extension mode and switches back.
- `extension/` at the repo root is the Chrome extension source. It has its own `manifest.json`, build scripts, and `PRIVACY.md`.

## Provider routing

`ProviderRegistry` (in `crates/api/src/provider/mod.rs`) owns the model catalog and routes to the correct client:

- If `credentials.json` has an `active_provider`, that provider is used regardless of model name.
- The model string must use `provider/model-id` format (e.g. `anthropic/claude-sonnet-4-6`). `provider_for_model` extracts the provider prefix; `model_api_id` strips it to get the raw API ID.
- `build_client` constructs an `Anthropic`, `OpenAi`, or `Custom` (OpenAI-compatible chat/completions) client from the stored `StoredProviderConfig` for that provider.

Default model comes from the `default_model` field in the active provider's `StoredProviderConfig` inside `credentials.json`. `--model` on the CLI overrides it.

## Tool surface

`agent::mvp_tool_specs()` returns the canonical 21-tool list with JSON schemas and required permission. When you add or rename a tool, update `mvp_tool_specs`, add a handler in `tools/mod.rs`, and adjust the count assertion in `crates/agent/src/lib.rs` tests.

## Conventions specific to this repo

- **Always run `cargo fmt` before committing.** CI checks formatting with `cargo fmt --check` — commits that fail this check will be rejected.
- `unsafe_code = "forbid"` at the workspace level — do not introduce `unsafe`.
- Clippy `pedantic` is on as a warning; `module_name_repetitions`, `missing_panics_doc`, `missing_errors_doc` are explicitly allowed. New lint warnings should be fixed rather than suppressed locally unless there's a reason.
- Tests that mutate process env (provider, model, workspace dir) must serialize with a `OnceLock<Mutex<()>>` guard, following the pattern in `cli/src/main.rs` and `crates/runtime/src/lib.rs::test_env_lock`.
- Slash-command behavior is shared between the live REPL and `--resume`. When editing a slash command, check `resume_supported_slash_commands()` — the test `resume_supported_command_list_matches_expected_surface` pins the exact resume-safe set.
- TUI popup/list UX baseline (applies to slash overlay + auth modal lists + similar list selectors):
  - Keep one blank line at the top of popup content.
  - Keep key-hint text pinned to the last visible content row, with a blank separator row above it and no extra blank row below it; style hints in dim gray.
  - Up/Down navigation must clamp at edges (no wrap-around) for both keyboard and mouse wheel.
  - For list selectors, Left jumps to the first item and Right jumps to the last item.
  - When scrolling to keep selection visible, use edge-follow behavior (no forced centering jumps).

## Releasing a new version

1. Bump `version` in the root `Cargo.toml` (workspace-level — all crates inherit via `version.workspace = true`).
2. Add a `## [X.Y.Z] - YYYY-MM-DD` section to `CHANGELOG.md` following the Keep a Changelog format. The release workflow extracts this section verbatim as the GitHub Release body. **Also add the corresponding reference link at the bottom of the file:** `[X.Y.Z]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/vX.Y.Z`
3. Run `cargo check` to regenerate `Cargo.lock` (CI builds with `--locked`).
4. Commit both files: `git commit -am "chore: bump version to X.Y.Z"`
5. Tag at the version-bump commit: `git tag vX.Y.Z`
6. Push both: `git push origin main && git push origin vX.Y.Z`

The tag-triggered workflow (`.github/workflows/release.yml`) builds binaries for 5 targets (linux x64/arm64, macos x64/arm64, windows x64), generates `checksums.sha256`, checks out `CHANGELOG.md`, extracts the section for the tagged version, and creates a GitHub Release with the changelog text as the body and all artifacts attached.

**Important:** The tag must point at the commit that contains the version bump. If you tag before bumping, the compiled binary will report the old version via `env!("CARGO_PKG_VERSION")`. If you need to fix a mis-tagged release, delete the remote tag (`git push origin --delete vX.Y.Z`), delete local (`git tag -d vX.Y.Z`), re-tag at the correct commit, and push again.

**CHANGELOG format:** Each version section must start with `## [X.Y.Z]` on its own line. The workflow uses `awk` to extract everything between that header and the next `## [` line. If no matching section is found, the release body falls back to "Release vX.Y.Z".

<!-- code-review-graph MCP tools -->
## MCP Tools: code-review-graph

**IMPORTANT: This project has a knowledge graph. ALWAYS use the
code-review-graph MCP tools BEFORE using Grep/Glob/Read to explore
the codebase.** The graph is faster, cheaper (fewer tokens), and gives
you structural context (callers, dependents, test coverage) that file
scanning cannot.

### When to use graph tools FIRST

- **Exploring code**: `semantic_search_nodes` or `query_graph` instead of Grep
- **Understanding impact**: `get_impact_radius` instead of manually tracing imports
- **Code review**: `detect_changes` + `get_review_context` instead of reading entire files
- **Finding relationships**: `query_graph` with callers_of/callees_of/imports_of/tests_for
- **Architecture questions**: `get_architecture_overview` + `list_communities`

Fall back to Grep/Glob/Read **only** when the graph doesn't cover what you need.

### Key Tools

| Tool | Use when |
|------|----------|
| `detect_changes` | Reviewing code changes — gives risk-scored analysis |
| `get_review_context` | Need source snippets for review — token-efficient |
| `get_impact_radius` | Understanding blast radius of a change |
| `get_affected_flows` | Finding which execution paths are impacted |
| `query_graph` | Tracing callers, callees, imports, tests, dependencies |
| `semantic_search_nodes` | Finding functions/classes by name or keyword |
| `get_architecture_overview` | Understanding high-level codebase structure |
| `refactor_tool` | Planning renames, finding dead code |

### Workflow

1. The graph auto-updates on file changes (via hooks).
2. Use `detect_changes` for code review.
3. Use `get_affected_flows` to understand impact.
4. Use `query_graph` pattern="tests_for" to check coverage.
