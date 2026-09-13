# acrawl MCP — Setup & Troubleshooting

**Contents**

- [Prerequisites](#prerequisites)
- [Install acrawl](#install-acrawl)
- [Register the MCP server](#register-the-mcp-server)
- [Credentials (run_goal only)](#credentials-run_goal-only)
- [Settings](#settings)
- [Browser backends](#browser-backends)
- [Verify the install](#verify-the-install)
- [Troubleshooting](#troubleshooting)

---

## Prerequisites

- The `acrawl` binary on `PATH`.
- **Node.js 20+** — CloakBrowser (the stealth Chromium driver) runs as an embedded Node subprocess. The browser binary itself auto-downloads on first use; there is no separate install step.
- LLM credentials **only for `run_goal`**. The other 38 tools need no configuration at all.

---

## Install acrawl

**Linux / macOS:**

```bash
curl -fsSL https://raw.githubusercontent.com/Mingye-Lu/AgenticCrawler/main/install.sh | bash
```

**Windows (PowerShell):**

```powershell
irm https://raw.githubusercontent.com/Mingye-Lu/AgenticCrawler/main/install.ps1 | iex
```

Both download the binary, verify its SHA256, and set up CloakBrowser.

**From source:**

```bash
git clone https://github.com/Mingye-Lu/AgenticCrawler.git
cd AgenticCrawler
cargo build --release   # -> ./target/release/acrawl
npm install             # CloakBrowser
```

---

## Register the MCP server

Easiest path — auto-detects installed IDEs and writes the right config for each:

```bash
acrawl mcp install
```

Non-interactive variants:

```bash
acrawl mcp install --client opencode --scope user
acrawl mcp install --all --yes
```

Supported clients (17): Claude Code, Claude Desktop, Cursor, Windsurf, VS Code (Copilot), OpenCode, Zed, TRAE, JetBrains IDEs, Gemini CLI, Qwen Code, Codex CLI, Hermes, OpenClaw, Goose, Crush, Aider.

### Manual config

Most clients (Claude Code `.mcp.json`, Cursor `.cursor/mcp.json`, Windsurf, Claude Desktop, TRAE, Gemini CLI, Qwen Code):

```json
{
  "mcpServers": {
    "acrawl": { "command": "acrawl", "args": ["mcp"] }
  }
}
```

VS Code (`.vscode/mcp.json`) uses `servers` instead of `mcpServers`:

```json
{
  "servers": {
    "acrawl": { "command": "acrawl", "args": ["mcp"] }
  }
}
```

OpenCode (`opencode.json`):

```json
{
  "mcp": {
    "acrawl": { "type": "local", "command": ["acrawl", "mcp"] }
  }
}
```

Zed (`~/.config/zed/settings.json`):

```json
{
  "context_servers": {
    "acrawl": {
      "command": { "path": "acrawl", "args": ["mcp"], "env": {} },
      "settings": {}
    }
  }
}
```

Claude Code CLI one-liner:

```bash
claude mcp add acrawl -- acrawl mcp
```

**Transport is stdio only.** No SSE, HTTP, or WebSocket in this release.

---

## Credentials (`run_goal` only)

Stored at `~/.acrawl/credentials.json`. Skip this entirely unless you need `run_goal`.

`run_goal` needs **two** pieces of configuration, and credentials alone are not enough:

```bash
acrawl auth anthropic --api-key "sk-ant-..."          # 1. credentials
acrawl config set model anthropic/claude-sonnet-4-6   # 2. settings.model  <- also required
```

An omitted `model` in the request is resolved *only* from `settings.json`'s prefixed `model` field. The active provider's `default_model` inside `credentials.json` is **not** consulted, so configuring credentials without `acrawl config set model` still yields `no model configured`. (The error text suggests `acrawl auth`, which is misleading.) Alternatively, pass `model` explicitly on every `run_goal` call.

Other providers follow the same pattern:

```bash
acrawl auth openai --api-key "sk-..."
acrawl auth amazon-bedrock --access-key AKIA... --secret-key ... --region us-east-1
acrawl auth other --base-url http://localhost:11434/v1   # Ollama, no key
```

Models use `provider/model-id` — e.g. `anthropic/claude-sonnet-4-6`, `openai/gpt-4o`, `other/llama3.2`.

Gate on readiness in scripts and CI:

```bash
acrawl auth status --check anthropic   # exit 0 = ready, 3 = not configured
acrawl auth status --json              # full table, secrets masked
```

Exit codes: `0` ok · `1` error · `2` usage/config · `3` not-configured.

File shape:

```json
{
  "active_provider": "anthropic",
  "providers": {
    "anthropic": {
      "auth_method": "api_key",
      "api_key": "sk-ant-...",
      "default_model": "claude-sonnet-4-6"
    }
  }
}
```

`auth_method` is `api_key`, `oauth`, or `aws_sigv4`. Provider-specific extras: Azure needs `resource_name` + `deployment_name`; Bedrock needs `region` + `aws_secret_access_key` (+ `session_token` for STS); Vertex needs `gcp_project_id` + `gcp_region`; custom endpoints use `base_url`.

Routing is driven by the model string's `provider/` prefix, not by `active_provider`: `openai/gpt-4o` goes to OpenAI even when `active_provider` is `anthropic`. `active_provider` selects defaults elsewhere but does not override an explicit prefix. 25 providers are supported; `acrawl auth list` enumerates them.

---

## Settings

`~/.acrawl/settings.json`, created with defaults on first run. Every field is optional.

```bash
acrawl config get headless
acrawl config set headless false
acrawl config get model --effective
```

Relevant to MCP use:

| Field | Default | Notes |
|---|---|---|
| `headless` | `true` | `false` gives a visible browser — the main CAPTCHA workaround |
| `output_dir` | `"output"` | where `save_file` / `screenshot(save: true)` write |
| `max_steps` | `50` | agent loop cap. **Not read by `run_goal` over MCP** - pass `max_steps` in the request instead |
| `browser_backend` | `null` | `"extension"` or `null` (CloakBrowser) |
| `extension_bridge_port` | `19876` | extension bridge WebSocket port |

Script limits under `script` — see `scripting.md` for the full table:

```json
{
  "script": {
    "max_steps": 200,
    "max_timeout_secs": 300,
    "per_step_timeout_secs": 30,
    "max_output_bytes": 10485760,
    "max_concurrent_scripts": 5
  }
}
```

### Optimizations — three default ON

Under `optimization`. Contrary to README text saying everything defaults off, `OptimizationSettings::default()` enables three:

| Field | Default | Effect |
|---|---|---|
| `failure_classification` | **`true`** | sorts errors into 16 categories (SelectorNotFound, CaptchaDetected, RateLimited, …) by keyword; no LLM cost |
| `self_healing` | **`true`** | on SelectorNotFound/Ambiguous, re-fetches page_map and text-matches a replacement; logs `[healed: @eOLD -> @eNEW]` |
| `content_aware_profiles` | **`true`** | picks a cleaning profile from the task: ReadingMode for extraction, Minimal for interaction, Aggressive above 50 KB |

Self-healing is why a selector you expected to fail sometimes works. Set `"self_healing": false` when you specifically want strict selector behaviour.

Everything else defaults off — notably `html_diff_mode` (50–70% token cut on repeat visits), `action_caching`, `loop_detection`, `page_fingerprinting`, and the `budget_*` controls. A reasonable cost-conscious profile:

```json
{
  "optimization": {
    "html_diff_mode": true,
    "page_fingerprinting": true,
    "action_caching": true,
    "loop_detection": true,
    "budget_max_session_cost_usd": 0.50,
    "budget_enforcement": "warn"
  }
}
```

`action_caching` needs `page_fingerprinting` — the cache key is tool + input + page fingerprint.

Override the whole config directory with `ACRAWL_CONFIG_HOME`. Useful for isolating a test profile.

---

## Browser backends

**CloakBrowser is the only backend available to MCP calls.** The MCP server constructs a `PlaywrightBridge` directly and never reads the `browser_backend` setting, so the rest of this section applies to the REPL, not to tools you call over MCP.

**CloakBrowser (default)** — headless stealth Chromium, auto-downloaded, driven by an embedded Node subprocess.

**Extension bridge (REPL only)** — acrawl drives *your real browser* over CDP through a local WebSocket, inheriting your cookies, sessions, and extensions. That makes it the answer for login-walled content and for sites whose bot detection defeats headless — but **only from the REPL**. Setting `browser_backend: "extension"` or running `/extension` does not change which backend an MCP tool call uses; an MCP session cannot inherit your real browser's session.

Setup: download `acrawl-extension.zip` from the latest release, unzip, load unpacked at `chrome://extensions` (or `edge://`, `brave://`) with Developer mode on, then run `/extension` in the acrawl REPL to start the bridge and print the auth token.

The bridge listens on `127.0.0.1:19876`, uses a 256-bit hex token compared in constant time, validates the extension-ID origin, and accepts one client at a time. Extension mode activates only when the extension actually connects, not when the server starts. `/cloakbrowser` switches back.

For a login-walled target over MCP, the options are to authenticate within the MCP session via `fill_form`, or to drive the site from the REPL in extension mode instead.

---

## Verify the install

```bash
acrawl mcp        # should start and wait on stdio (Ctrl+C to exit)
```

From an MCP client, the cheapest end-to-end check needs no credentials and no browser:

1. `list_scripts` — exercises the server with zero side effects.
2. `run_script` with a computation-only script, then `wait_for_scripts`:

```json
{"schema_version": 1,
 "steps": [{"type": "assign", "variable": "v", "value": {"kind": "literal", "value": "ok"}},
           {"type": "collect", "value": {"kind": "variable", "value": "v"}}]}
```

Expect `status: "Completed"`, `extracted_data: ["ok"]`, `steps_executed: 0`, elapsed ~1 ms. This confirms server, parser, and executor health. It does **not** bypass the browser: over MCP every `run_script` launches the browser before parsing, so a failure here can still mean Node or Chromium is broken rather than the server.

3. `navigate(url: "https://example.com", content_depth: "none", page_map_depth: "none")` — first real browser exercise; may be slow while Chromium downloads.

---

## Troubleshooting

**`missing field schema_version`** — you used `version: "1.0"` from the `run_script` description. The server requires `schema_version` as an **integer** (`1`). See `scripting.md`.

**`undefined variable \`x\`` at submit time** — the script parser validates statically before executing. Every variable must be assigned or bound by an enclosing loop/`error_var` first. Note `save_script` does *not* run this check, so saving successfully proves nothing about validity.

**`invalid limits: missing field ...`** — `limits` is an all-or-nothing object. Supply all five required fields (`max_steps`, `max_timeout_secs`, `per_step_timeout_secs`, `max_output_bytes`, `max_parallel_branches`), not just the one you want to change.

**`run_script(name: "...")` fails** — loading a saved script by name is not wired up on the MCP path. `read_script` it and pass the JSON as `script` instead.

**`save_as` produced no file** — it is parsed but never persisted over MCP. Call `save_script` explicitly.

**Script returned data from the wrong page** — you ran scripts concurrently. All concurrent scripts share the session's single tab. Run them one at a time.

**`unknown tool` for a valid-looking name** — names are matched verbatim over MCP, so `read-content` is rejected. Use underscores.

**`unknown tool` inside a script** — scripts may call only 17 of the 31 browser tools. All DevTools tools plus `refresh` and `set_device` are excluded; run those manually.

**`failed to close parallel pages: Invalid page index N`** — the `parallel` node is broken for 2+ branches. Use several concurrent `run_script` calls joined by one `wait_for_scripts`.

**`run_goal` fails but browser tools work** — credentials aren't configured. `acrawl auth status --check <provider>` (exit 3 = not configured). Retrying won't help; configure, or use manual tools / a script.

**Empty content on a JS-heavy page** — the HTTP tier returned the shell. `navigate` escalates on framework markers, but slow hydration still needs `wait(selector: ..., state: "visible")` before reading. Also check you didn't set `content_depth: "none"`.

**`changed: false` after an action** — a no-op. Don't retry the same call: re-read `page_map` for fresh `@eN` refs, or target by `text` + `role`. After a submit, check `list_page_logs(level: "error")` for validation failures.

**Interception has no effect** — rules apply to future requests. Call `refresh()` after adding them.

**Stale/incorrect `inspect_request` result** — `@rN` is positional within the most recent listing, not a stable ID. Re-list, then inspect immediately.

**CAPTCHA / bot wall** - `acrawl config set headless false` (or `--headed`), then restart the MCP server. Extension mode is not available to MCP calls. Invisible reCAPTCHA v3 surfaces as a `CaptchaDetected` error when a submit produces no page change; that is a best-effort heuristic since the server-side score is unreadable. Retrying headless will not help.

**2FA** - acrawl can type a code you supply but cannot receive SMS or generate TOTP. Over MCP there is no way to reuse an already-authenticated real browser; drive such sites from the REPL in extension mode instead.

**Saved PDF has no readable text** — `save_file` downloads bytes; acrawl doesn't extract PDF text. Navigate to an HTML version or process the file externally.

**Browser won't launch** — confirm Node.js 20+ (`node --version`). The Chromium download happens on first browser use and can take a while.
