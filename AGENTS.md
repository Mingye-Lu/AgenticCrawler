# AGENTS.md

## Project

`acrawl` is a native-Rust LLM-driven web crawler. A user provides a natural-language goal; the agent plans, navigates, and extracts structured data via a 42-tool toolbox (31 browser + 4 agent-control + 7 script). It ships as a single binary with three modes: an interactive Ratatui TUI REPL (requires a TTY), non-interactive `prompt` (one-shot) / `--resume` (slash-command replay), and `mcp` (built-in MCP server over stdio).

## Commands

```bash
cargo build --release                                        # produce ./target/release/acrawl
cargo test --workspace                                       # run full test suite (~1,100 tests)
cargo test -p <crate> <test_name>                            # run a single test (e.g. -p agent mvp_tool_specs_contains_expected_42_tools)
cargo clippy --workspace --all-targets -- -D warnings        # lints must be clean (workspace lints set pedantic = warn)
cargo fmt --check                                            # format check

./target/release/acrawl                                      # launch REPL
./target/release/acrawl prompt "scrape all titles from example.com"   # one-shot
./target/release/acrawl mcp                                  # launch MCP server (stdio)
./target/release/acrawl mcp install                          # interactive IDE installer
./target/release/acrawl --resume session.json /status /compact        # non-interactive session maintenance

# Non-interactive provider setup (agent/CI use)
acrawl auth anthropic --api-key "sk-ant-..."      # configure credentials
acrawl auth openai --api-key "sk-..."             # other providers same pattern
acrawl auth amazon-bedrock --access-key AKIA... --secret-key ... --region us-east-1
acrawl config set model anthropic/claude-sonnet-4-6  # set default model
acrawl auth status --check anthropic              # gate: exit 0 if ready, 3 if not
acrawl auth status                                # show all configured providers
acrawl auth list                                  # list all available providers
acrawl config get headless                        # read a setting
acrawl config set headless false                  # write a setting
acrawl mcp install --client opencode             # install MCP for one IDE
acrawl mcp install --all --yes                   # install for all IDEs
# Exit codes: 0=ok  1=error  2=usage/config  3=not-configured
```

The CLI reads LLM credentials from `~/.acrawl/credentials.json` (managed by `acrawl auth`) and runtime settings from `~/.acrawl/settings.json`. Both paths respect the `ACRAWL_CONFIG_HOME` env var override. Run `acrawl auth [anthropic|openai|other]` to configure a provider.

## Workspace layout

Eleven crates under `crates/`, compiled with `resolver = "2"`:

- **core** (`acrawl-core`) — shared types, traits, and error hierarchy used across the workspace. Defines `ToolSpec`, `ToolEffect`, `AssistantEvent`, `RuntimeObserver`, `ContentBlock`/`ConversationMessage`/`MessageRole`/`TokenUsage`, `ToolOutcome`, `ApiClient`/`ApiRequest`, `config_home_dir`, and `OAuthConfig`.
- **api** — HTTP + SSE clients for Anthropic (`client.rs`), OpenAI-compatible (`openai.rs`), and Codex OAuth (`codex.rs`). `sse.rs` is the shared streaming frame parser; `types.rs` holds the Anthropic message schema. `oauth.rs` contains OAuth PKCE helpers, credential persistence, and token exchange types. `provider/registry.rs` and `provider/factory.rs` handle provider discovery and client construction.
- **browser** — browser automation layer. `PlaywrightBridge` (CloakBrowser headless Chromium), `ExtensionBridge` (Chrome extension backend via CDP), `FetchRouter` (HTTP→browser escalation), `BrowserContext` (tab/URL state), and `WsBridgeServer` (WebSocket server for extension communication). `browser_backend.rs` defines the `BrowserBackend` trait that both bridges implement.
- **agent** — agent orchestration and the 42-tool toolbox (31 browser + 4 agent-control + 7 script). `agent.rs` drives the agent loop; `tools/` contains individual tool handlers; `manager.rs` manages sub-agent fork/join lifecycle; `prompt.rs` builds the system prompt; `state.rs` holds `CrawlState`; `url_claim.rs` coordinates URL claims across agents.
- **runtime** — `ConversationRuntime` (the core turn loop), `Session` persistence, system-prompt builder, compaction, usage/pricing, `config/` subdirectory (loader, MCP config, features), and a full MCP client stack in `mcp/` (`client.rs`, `types.rs`, `server_manager.rs`, `process.rs`, `naming.rs`).
- **render** — markdown/terminal rendering (`markdown.rs`), tool call output formatting (`tool_format.rs`), output format selection (`format.rs`), and the `OutputSink` trait + implementations (`sink.rs`) that bridge runtime events to the UI.
- **mcp-server** — built-in MCP server (`server.rs`: JSON-RPC over stdio, 31 direct browser tools + 7 script tools + `run_goal`) and the interactive IDE installer (`installer.rs`: `acrawl mcp install`). Supports 17 clients: Claude Code, Claude Desktop, Cursor, Windsurf, VS Code, OpenCode, Zed, TRAE, JetBrains, Gemini CLI, Qwen Code, Codex CLI, Hermes, OpenClaw, Goose, Crush, Aider.
- **tui** (`acrawl-tui`) — Ratatui terminal UI. `repl_app/` (directory) owns the application state; `repl_render.rs` handles rendering; `modals/` contains auth, model-picker, and slash-command overlay widgets. Depends on `acrawl-ui`.
- **ui** (`acrawl-ui`) — shared application layer used by both TUI and CLI. Owns `LiveCli`, provider code paths (`api_client.rs`, `tool_executor.rs`, `model_support.rs`, `runtime_builder.rs`, `resume.rs`), session management (`session_mgr.rs`), output sink (`output_sink.rs`), and auth helpers.
- **cli** — thin binary entry point (`main.rs`). `self_update.rs` handles `acrawl update`; `uninstall.rs` handles `acrawl uninstall`. All orchestration and session management live in `acrawl-ui`.
- **commands** — slash-command registry (`/help`, `/status`, `/model`, `/compact`, `/clear`, `/cost`, `/sessions`, `/export`, `/config`, `/auth`, `/headed`, `/headless`, `/extension`, `/cloakbrowser`, `/debug`, `/version`, `/exit`). Knows which commands are safe to replay in `--resume`.

## Architecture: how a turn actually flows

1. `cli::app::LiveCli` builds a `ProviderClient` via `ProviderRegistry` from the persisted `CredentialStore` (`credentials.json`), plus a `ToolExecutor` backed by `agent::ToolRegistry`.
2. `runtime::ConversationRuntime::run_turn` drives the loop: call `ApiClient::stream` → feed `AssistantEvent`s (text deltas, tool_use, usage, stop) → execute tools through `ToolExecutor` → append results → repeat until the model emits `MessageStop` with no tool calls or `MAX_STEPS` is hit. The runtime notifies a `RuntimeObserver` at each event (text deltas, tool calls, turn end); `OutputSink` (`StdoutSink` for non-interactive `prompt`/`--resume`, `ChannelSink` for TUI) implements this trait to bridge events to the UI.
3. The crawler tool handlers (`crates/agent/src/tools/*.rs`) take JSON input, consult `CrawlState`, and act through a `BrowserContext` that wraps either the `FetchRouter` (reqwest HTTP path) or the `PlaywrightBridge` (headless Chromium). The router auto-escalates from HTTP to the browser when JS is needed.
4. The optional `--allowedTools` CLI flag restricts which tools are available; `CliToolExecutor` enforces this before execution. `ToolSpec` has no permission tier — all 42 tools are unrestricted by default.
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

`agent::mvp_tool_specs()` returns the canonical 42-tool list with JSON schemas and required permission. When you add or rename a tool, update `mvp_tool_specs`, add a handler in `tools/mod.rs`, and adjust the count assertion in `crates/agent/src/lib.rs` tests.

## Optimization layer

14 vendor-derived optimizations live in `crates/agent/src/` and `crates/runtime/src/`. All are gated by `settings.optimization.*` fields (all default OFF). The pattern every optimization follows:

### Shared infrastructure (must understand before touching any optimization)

**`DynamicPromptContext`** (`crates/agent/src/prompt.rs`) — four optional string fields (`stagnation_alert`, `planning_guidance`, `budget_warning`, `loop_nudge`). `build_system_prompt(specs, Some(&ctx))` appends the context as section 9 of the system prompt.

**Arc slot pattern** — `CrawlerAgent` and `ConversationRuntime` share two Arc slots created in `run_with_system_prompt()`:
- `prompt_override: Arc<Mutex<Option<Vec<String>>>>` — agent writes a new full system prompt here after any tool execution; runtime applies it before the next API call in `prepare_iteration()`.
- `last_assistant_text: Arc<Mutex<Option<String>>>` — runtime writes the latest assistant response text here; agent reads it for confidence parsing.
- `cumulative_cost: Arc<AtomicU64>` (millicents) — runtime updates it after each usage record; agent reads it for budget enforcement.

All three slots are internal to `ConversationRuntime` (not constructor parameters) but accessible via getters. The agent gets the cost counter via `runtime.cumulative_cost_counter()` after construction.

### Per-optimization modules

| Module | Location | What it adds to `CrawlState` / `CrawlerAgent` |
|--------|----------|-----------------------------------------------|
| `page_fingerprint` | `crates/agent/src/page_fingerprint.rs` | `CrawlState.page_fingerprints: Vec<PageFingerprint>` |
| `tools/html_diff` | `crates/agent/src/tools/html_diff.rs` | `CrawlState.html_diff_tracker: Option<HtmlDiffTracker>` |
| `loop_detector` | `crates/agent/src/loop_detector.rs` | `CrawlState.loop_detector: Option<LoopDetector>` |
| `failure_classifier` | `crates/agent/src/failure_classifier.rs` | (pure function — no state) |
| `self_healing` | `crates/agent/src/self_healing.rs` | (pure function — no state) |
| `action_cache` | `crates/agent/src/action_cache.rs` | `CrawlState.action_cache: Option<ActionCache>` |
| `confidence` | `crates/agent/src/confidence.rs` | `CrawlerAgent.confidence_tracker: Option<ConfidenceTracker>` |
| `budget` | `crates/runtime/src/budget.rs` | `CrawlerAgent.cumulative_cost_slot: SharedCostCounter` |

### Where optimizations run

All optimization logic runs inside `CrawlerAgent::execute()` in `crates/agent/src/implementation/mod.rs`. The execution order (each guarded by its settings flag):
1. **Action cache lookup** — before the tool runs (returns cached result if hit)
2. **Tool execution** — normal handler dispatch
3. **Self-healing retry** — on SelectorNotFound/SelectorAmbiguous
4. **Loop detection** — records action + fingerprint, writes nudge to prompt_override_slot
5. **Planning interval** — injects planning/execution guidance at step N
6. **Confidence tracking** — reads last_assistant_text slot, parses `[confidence: ...]`
7. **Budget enforcement** — reads cumulative_cost_slot, warns or blocks
8. **Action cache store** — stores result after successful read-only tool call

`CrawlState` fields are ephemeral (never persisted to session files). Adding a new field requires no serde changes.

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

## Development workflow

- **Worktree setup.** Before starting any feature, fix, or refactor, create a dedicated worktree under `.worktrees/` on a new semantically named branch — use the `feat/`, `fix/`, `chore/`, or `docs/` prefix followed by a short hyphenated description:
  ```bash
  git worktree add .worktrees/<branch-name> -b <branch-name>
  # e.g. git worktree add .worktrees/feat/aria-snapshot -b feat/aria-snapshot
  ```
  Do all development inside that worktree directory. The only commits permitted directly on `main` are version bumps (see [Releasing a new version](#releasing-a-new-version)).
- **Atomic commits.** Each commit must be a single, self-contained logical change — individually revertible without pulling in unrelated work. Stage precisely and avoid bulk commits that bundle multiple concerns.
- **Commit messages.** Format: `type(scope): imperative summary` (≤72 chars), `type` one of `feat|fix|perf|refactor|test|style|docs|ci|chore`. The message describes *what changed and why*, as it would read to someone who has never seen the review thread or your working session — never the process that produced the commit.
  - **Never reference a PR number, review round, or reviewer in the subject line.** `fix: address review comments on PR #109 — …` and `fix: apply cap to selector waits too (PR #101 review)` are both wrong — they document the review process, not the change. Write what the commit actually does: `fix: avoid browser launch, skip seq increment, propagate eval errors`. (`Closes #N` / `Fixes #N` issue-closing footers are fine — they're not review-process narration.)
  - **If you're revising a commit in response to review feedback on your own not-yet-merged branch, rewrite the original commit** (`git commit --amend`, or reword/fixup via rebase) instead of stacking a new commit that narrates "addressed review comments." The final history should read as if it were written correctly the first time.
  - **Never let tool, API, or auth failures leak into the message.** If whatever step you're using to compose the message fails or times out, stop and write the message yourself from the diff — do not append or fall back to raw diagnostic text (e.g. `Claude Code was unavailable (OAuth 401).`) as if it were content. A failed message-generation step is a reason to retry or write it manually, never a reason to commit the error.
- **PR workflow.** When a branch is complete: write the PR title and body to a temporary file (e.g. `pr.md`), submit via `gh pr create --title "…" --body "$(cat pr.md)"`, then delete the file. Keep the title under 70 characters; include a concise summary and a markdown test-plan checklist in the body.
- **Worktree cleanup.** Once the PR is open, remove the worktree and delete the local branch — the remote branch stays alive until the PR is merged:
  ```bash
  git worktree remove .worktrees/<branch-name>   # unmount and delete the directory
  git branch -d <branch-name>                    # delete the local branch
  git worktree prune                             # remove any stale worktree metadata
  ```
- **Merging (admin: Mingye-Lu only).** Merge via standard merge commit — never squash. Delete the remote branch as part of the merge:
  ```bash
  gh pr merge <PR-number> --merge --delete-branch   # merge commit, delete remote branch
  git fetch --prune                                  # drop stale remote-tracking refs locally
  ```

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
