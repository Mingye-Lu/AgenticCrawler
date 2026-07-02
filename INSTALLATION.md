# Installation

## One-line install

**Linux / macOS**

```bash
curl -fsSL https://raw.githubusercontent.com/Mingye-Lu/AgenticCrawler/main/install.sh | bash
```

Downloads the latest release for your platform, verifies the SHA-256 checksum, installs to `~/.local/bin/acrawl`, and sets up CloakBrowser automatically if Node.js 20+ is present.

| Variable | Default | Description |
|----------|---------|-------------|
| `ACRAWL_INSTALL_DIR` | `~/.local/bin` | Where to place the `acrawl` binary |
| `ACRAWL_CONFIG_HOME` | `~/.acrawl` | Config and browser install directory |

```bash
ACRAWL_INSTALL_DIR=/usr/local/bin \
  curl -fsSL https://raw.githubusercontent.com/Mingye-Lu/AgenticCrawler/main/install.sh | bash
```

**Windows (PowerShell 5.1+)**

```powershell
irm https://raw.githubusercontent.com/Mingye-Lu/AgenticCrawler/main/install.ps1 | iex
```

Installs to `$HOME\.acrawl\bin\acrawl.exe` and adds that directory to the user PATH. Restart your terminal after installation for PATH changes to take effect.

| Variable | Default | Description |
|----------|---------|-------------|
| `ACRAWL_CONFIG_HOME` | `$HOME\.acrawl` | Config and browser install directory (binary goes into `bin\` inside this) |

## Pre-built binary (manual)

Download the binary for your platform from the [GitHub Releases](https://github.com/Mingye-Lu/AgenticCrawler/releases) page.

| Platform        | Artifact                  |
|-----------------|---------------------------|
| Linux x64       | `acrawl-linux-x64`        |
| Linux arm64     | `acrawl-linux-arm64`      |
| macOS x64       | `acrawl-macos-x64`        |
| macOS arm64     | `acrawl-macos-arm64`      |
| Windows x64     | `acrawl-windows-x64.exe`  |

Place the binary on your `PATH` and make it executable:

```bash
# Linux / macOS
chmod +x acrawl-<platform>
mv acrawl-<platform> /usr/local/bin/acrawl

# Windows — rename to acrawl.exe and add its directory to PATH
```

## Build from source

Requires the [Rust toolchain](https://rustup.rs) (stable).

```bash
git clone https://github.com/Mingye-Lu/AgenticCrawler.git
cd AgenticCrawler
cargo build --release
# Binary: ./target/release/acrawl
```

## Browser prerequisites

Browser automation uses **CloakBrowser** (stealth headless Chromium) which requires **Node.js 20+**.

```bash
node --version   # must be v20 or newer
# Install from https://nodejs.org/ if needed
```

Install the browser once Node.js is ready:

```bash
acrawl install-browser
```

This installs `cloakbrowser` and `playwright-core` into `~/.acrawl/node_modules/` and downloads the Chromium binary. On Linux it also installs system-level Chromium dependencies (may require root). The browser binary also auto-downloads on first use if this step is skipped.

Alternatively, install the [Chrome extension](#chrome-extension-backend) to drive your own installed browser — no Node.js required for that mode.

## Configure a provider

Credentials are stored in `~/.acrawl/credentials.json` (override directory with `ACRAWL_CONFIG_HOME`).

### Interactive setup (first-time)

```bash
acrawl auth                    # pick a provider interactively
acrawl auth anthropic          # go straight to Anthropic setup
acrawl auth openai             # go straight to OpenAI setup
```

### Non-interactive setup (CI / scripting)

```bash
# Anthropic
acrawl auth anthropic --api-key "sk-ant-..."

# OpenAI
acrawl auth openai --api-key "sk-..."

# Google Gemini
acrawl auth google --api-key "AIza..."

# Amazon Bedrock
acrawl auth amazon-bedrock \
  --access-key AKIA... --secret-key ... --region us-east-1

# Azure OpenAI
acrawl auth azure \
  --api-key "..." --resource-name my-resource --deployment-name gpt-4o

# Google Vertex AI
acrawl auth vertex \
  --api-key "..." --gcp-project my-project --gcp-region us-central1

# Custom / self-hosted OpenAI-compatible endpoint
acrawl auth other \
  --api-key "..." --base-url https://my-llm-server/v1

# Append --json for machine-readable output
acrawl auth anthropic --api-key "sk-ant-..." --json
```

> **Warning:** `--api-key` is visible in the process list and shell history.
> Prefer environment variables (see below) for shared machines.

### Set the default model

```bash
acrawl config set model anthropic/claude-sonnet-4-6
```

Models use `provider/model-id` format. Run `acrawl auth list` to see all providers with their supported model prefixes.

### Check provider status

```bash
acrawl auth status                        # show all configured providers
acrawl auth status --check anthropic      # exit 0 = ready, exit 3 = not configured
acrawl auth status --json                 # machine-readable
acrawl auth list                          # list all 25+ providers with env var names
acrawl auth list --json
```

### Environment variable credentials

Set the provider's environment variable and `acrawl` picks it up automatically — no `acrawl auth` step needed:

| Provider                   | ID                  | Environment variable          |
|----------------------------|---------------------|-------------------------------|
| Anthropic                  | `anthropic`         | `ANTHROPIC_API_KEY`           |
| OpenAI                     | `openai`            | `OPENAI_API_KEY`              |
| Google Gemini              | `google`            | `GEMINI_API_KEY`              |
| Amazon Bedrock             | `amazon-bedrock`    | `AWS_ACCESS_KEY_ID`           |
| Azure OpenAI               | `azure`             | `AZURE_OPENAI_API_KEY`        |
| Google Vertex AI           | `vertex`            | `GOOGLE_APPLICATION_CREDENTIALS` |
| GitHub Copilot             | `copilot`           | *(OAuth only)*                |
| Groq                       | `groq`              | `GROQ_API_KEY`                |
| Cerebras                   | `cerebras`          | `CEREBRAS_API_KEY`            |
| DeepInfra                  | `deepinfra`         | `DEEPINFRA_API_KEY`           |
| Together AI                | `togetherai`        | `TOGETHER_API_KEY`            |
| Perplexity                 | `perplexity`        | `PERPLEXITY_API_KEY`          |
| xAI                        | `xai`               | `XAI_API_KEY`                 |
| DeepSeek                   | `deepseek`          | `DEEPSEEK_API_KEY`            |
| Cohere                     | `cohere`            | `COHERE_API_KEY`              |
| Mistral AI                 | `mistral`           | `MISTRAL_API_KEY`             |
| OpenRouter                 | `openrouter`        | `OPENROUTER_API_KEY`          |
| Vercel AI                  | `vercel`            | `VERCEL_API_KEY`              |
| Venice AI                  | `venice`            | `VENICE_API_KEY`              |
| Alibaba / DashScope        | `alibaba`           | `DASHSCOPE_API_KEY`           |
| Cloudflare Workers AI      | `cloudflare`        | `CLOUDFLARE_API_TOKEN`        |
| Cloudflare AI Gateway      | `cloudflare-gateway`| `CLOUDFLARE_API_TOKEN`        |
| SAP AI Core                | `sap`               | `SAP_AI_CORE_API_KEY`         |
| GitLab Duo                 | `gitlab`            | `GITLAB_TOKEN`                |
| Other (custom endpoint)    | `other`             | *(set via `--base-url`)*      |

## Chrome extension backend

An alternative browser backend that drives your already-installed Chrome or Edge via CDP — no Chromium download or Node.js required.

1. Download `acrawl-extension.zip` from the [GitHub Releases](https://github.com/Mingye-Lu/AgenticCrawler/releases) page (or load `extension/` from the repo directly).
2. Open `chrome://extensions` in Chrome/Edge, enable **Developer mode**, click **Load unpacked**, and select the extracted folder.
3. Inside the `acrawl` REPL, run `/extension` to start the local bridge server and display the connection token.
4. Click the extension icon in the browser and paste the token.

Configure the bridge port (default `19876`) if needed:

```bash
acrawl config set extension_bridge_port 19876
```

## MCP server / IDE installation

```bash
acrawl mcp install                          # interactive: pick which clients
acrawl mcp install --all                    # install for all 17 supported clients
acrawl mcp install --all --yes              # non-interactive, no confirmation
acrawl mcp install --client cursor          # install for a specific client
acrawl mcp install --client cursor,windsurf # install for multiple clients
acrawl mcp install --scope project          # project-local config
acrawl mcp install --scope user             # user-global config (default)
acrawl mcp install --json                   # machine-readable output
acrawl mcp install --list-clients           # print all supported client keys

acrawl mcp uninstall                        # interactive removal
acrawl mcp uninstall --all                  # remove from all clients
acrawl mcp uninstall --client cursor
```

Supported clients (17): Claude Code, Claude Desktop, Cursor, Windsurf, VS Code, OpenCode, Zed, TRAE, JetBrains, Gemini CLI, Qwen Code, Codex CLI, Hermes, OpenClaw, Goose, Crush, Aider.

Client keys for `--client`: `claude-code`, `claude-desktop`, `cursor`, `windsurf`, `vscode`, `opencode`, `zed`, `trae`, `jetbrains`, `gemini-cli`, `qwen-code`, `codex-cli`, `hermes`, `openclaw`, `goose`, `crush`, `aider`.

### MCPB bundle (alternative)

A self-contained `.mcpb` bundle is available on the Releases page. Some clients support installing directly from a bundle file.

| Platform    | Bundle                          |
|-------------|----------------------------------|
| Linux x64   | `acrawl-mcp-linux-x64.mcpb`    |
| Linux arm64 | `acrawl-mcp-linux-arm64.mcpb`  |
| macOS x64   | `acrawl-mcp-macos-x64.mcpb`    |
| macOS arm64 | `acrawl-mcp-macos-arm64.mcpb`  |
| Windows x64 | `acrawl-mcp-windows-x64.mcpb`  |

## Configuration

Settings are stored in `~/.acrawl/settings.json` (override directory with `ACRAWL_CONFIG_HOME`).

```bash
acrawl config get                        # show all settings
acrawl config get headless               # read one setting
acrawl config get headless --effective   # show resolved value (with defaults applied)
acrawl config get --json                 # machine-readable
acrawl config set headless false         # write a setting
acrawl config set max_steps 100
acrawl config unset headless             # remove a setting (revert to default)
acrawl config path                       # print the path to settings.json
```

Dot-notation is used for nested keys:

```bash
acrawl config set optimization.html_diff_mode true
acrawl config set optimization.budget_max_session_cost_usd 5.0
acrawl config set script.max_steps 400
```

Settings can also be written directly to `settings.json`. Legacy location `~/.acrawl.json` is also loaded and merged.

### Core settings

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `headless` | bool | `true` | Run browser in headless (invisible) mode |
| `max_steps` | u32 | `50` | Maximum agent loop iterations per session |
| `model` | string | — | Default model in `provider/model-id` format |
| `reasoning_effort` | string | — | Reasoning effort for reasoning models: `"high"`, `"medium"`, `"low"` |
| `output_dir` | string | `"output"` | Directory for saved files (relative: resolved against `~/.acrawl/`) |
| `auto_compact_input_tokens` | u64 | `200000` | Auto-compact session when input tokens exceed this threshold |
| `browser_backend` | string | — | `"cloakbrowser"` (default) or `"extension"` |
| `extension_bridge_port` | u16 | `19876` | WebSocket port for the Chrome extension bridge |

### Sub-agent / fork settings

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `max_concurrent_per_parent` | u32 | `5` | Max concurrent sub-agents per parent |
| `max_fork_depth` | u32 | `3` | Max fork nesting depth |
| `max_total_agents` | u32 | `10` | Max total agents across all parents |
| `fork_child_max_steps` | u32 | `100` | Max steps for forked child agents |
| `fork_wait_timeout_secs` | u32 | `60` | Timeout (seconds) for `wait_for_subagents` |

### Compaction settings

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `compaction_prune_protect_tokens` | u64 | `40000` | Token window that protects recent messages from pruning |
| `compaction_prune_max_output_chars` | u64 | `2000` | Max chars for truncated tool outputs during compaction |
| `compaction_preserve_recent_tokens` | u64 | `80000` | Token budget for the preserved tail |
| `compaction_preserve_recent_messages_floor` | u32 | `2` | Minimum messages always preserved |
| `compaction_max_summary_chars` | u64 | `1200` | Max chars for the compacted summary |
| `compaction_llm_summarization` | bool | `false` | Enable LLM-powered summarization (uses extra tokens) |

### Script resource settings (`script.*`)

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `script.max_steps` | usize | `200` | Max execution steps per script |
| `script.max_timeout_secs` | u64 | `300` | Max total script execution time (seconds) |
| `script.max_output_bytes` | usize | `10485760` | Max script output size (10 MB) |
| `script.max_parallel_branches` | usize | `10` | Max parallel branches in one script |
| `script.max_concurrent_scripts` | usize | `5` | Max concurrently running scripts |
| `script.per_step_timeout_secs` | u64 | `30` | Timeout per individual script step (seconds) |
| `script.max_script_size_bytes` | usize | `1048576` | Max script source size (1 MB) |
| `script.max_nesting_depth` | usize | `10` | Max nesting depth for script calls |
| `script.scripts_dir` | path | `~/.acrawl/scripts/` | Directory for storing scripts |

### Optimization settings (`optimization.*`)

All optimization flags are off by default.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `optimization.html_diff_mode` | bool | `false` | Enable HTML diff for page change detection |
| `optimization.loop_detection` | bool | `false` | Detect and break out of agent action loops |
| `optimization.loop_detection_window` | usize | `20` | Steps to look back for loop detection |
| `optimization.loop_nudge_threshold` | usize | `5` | Loop repetitions before injecting a nudge |
| `optimization.page_fingerprinting` | bool | `false` | Enable page fingerprinting |
| `optimization.planning_interval` | usize | `0` | Inject planning guidance every N steps (0 = disabled) |
| `optimization.failure_classification` | bool | `true` ¹ | Classify tool failures for better retry logic |
| `optimization.self_healing` | bool | `true` ¹ | Auto-retry failed selectors with alternatives |
| `optimization.self_healing_max_retries` | usize | `2` | Max self-healing retries per failure |
| `optimization.action_caching` | bool | `false` | Cache results of read-only tool calls |
| `optimization.action_cache_ttl_secs` | u64 | `30` | Action cache TTL (seconds) |
| `optimization.confidence_tracking` | bool | `false` | Parse `[confidence: ...]` from assistant text |
| `optimization.compound_enrichment` | bool | `false` | Enable compound enrichment passes |
| `optimization.content_aware_profiles` | bool | `true` ¹ | Enable content-aware tool profiles |
| `optimization.budget_max_session_cost_usd` | f64 | — | Hard cost cap per session in USD (unset = unlimited) |
| `optimization.budget_enforcement` | string | — | `"warn"`, `"block"`, or `"route_down"` |
| `optimization.budget_warn_threshold_pct` | u32 | `80` | Warn when cost reaches this % of the budget cap |
| `optimization.per_agent_cost_tracking` | bool | `false` | Track cost per individual sub-agent |

¹ Enabled by default for fresh installs (no `settings.json`). Existing installs without an `optimization` block retain the previous behaviour (`false`).

### MCP server configuration (`mcpServers`)

External MCP servers can be added directly to `settings.json`:

```json
{
  "mcpServers": {
    "my-server": {
      "command": "uvx",
      "args": ["mcp-server-name"],
      "env": { "TOKEN": "secret" }
    },
    "remote-server": {
      "type": "http",
      "url": "https://example.com/mcp",
      "headers": { "Authorization": "Bearer token" }
    }
  }
}
```

### Environment variables

| Variable | Description |
|----------|-------------|
| `ACRAWL_CONFIG_HOME` | Override config directory (default: `~/.acrawl/`) |
| `HEADLESS` | Set to `true`/`false` to override the `headless` setting |

## Update

```bash
acrawl update
```

Downloads the latest binary from GitHub Releases, verifies its SHA-256 checksum, and replaces the running executable in-place. Also updates CloakBrowser if Node.js 20+ is installed. On Windows, the old binary is renamed to `acrawl.exe.old` before replacement.

## Uninstall

```bash
acrawl uninstall            # remove binary and node_modules
acrawl uninstall --purge    # also delete settings, credentials, and sessions
```

Prompts for confirmation. On Windows, `acrawl` also removes its directory from the user PATH.

## Verify

```bash
acrawl --version            # print version and build info
acrawl auth status          # list all configured providers
acrawl --help               # show available commands and flags
```

Exit codes: `0` = ok, `1` = error, `2` = usage/config error, `3` = provider not configured.
