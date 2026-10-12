# acrawl Bridge Extension

Connects [acrawl](https://github.com/Mingye-Lu/AgenticCrawler) to your Chromium browser for AI-driven web automation. This lets acrawl control your real browser (with your logged-in sessions, cookies, and extensions) instead of a headless CloakBrowser instance.

## Installation

### Chrome Web Store (recommended)

Install [acrawl Bridge from the Chrome Web Store](https://chromewebstore.google.com/detail/acrawl-bridge/nbkpmfofkdjcgkncmpicbdkjmddllcep) and click **Add to Chrome**. The same listing installs in Edge, Brave, Arc, Vivaldi, and Opera. Then continue with [Configuration](#configuration).

### Manual install from release

Only needed if you can't use the store, or want an unreleased build.

Download `acrawl-extension.zip` from the [latest release](https://github.com/Mingye-Lu/AgenticCrawler/releases/latest) and unzip it to a folder (e.g. `acrawl-extension/`).

**Google Chrome:**

1. Navigate to `chrome://extensions`
2. Enable **Developer mode** (toggle in the top-right corner)
3. Click **Load unpacked**
4. Select the unzipped folder

**Microsoft Edge:**

1. Navigate to `edge://extensions`
2. Enable **Developer mode** (toggle in the bottom-left)
3. Click **Load unpacked**
4. Select the unzipped folder

**Brave:**

1. Navigate to `brave://extensions`
2. Enable **Developer mode** (toggle in the top-right corner)
3. Click **Load unpacked**
4. Select the unzipped folder

**Arc / Vivaldi / Opera (Chromium-based):**

1. Open the browser's extension management page (typically `<browser>://extensions`)
2. Enable **Developer mode**
3. Click **Load unpacked** and select the unzipped folder

> **Note:** After a browser update you may see an "errors" badge on the extension. Simply click the extension icon or revisit the extensions page — the extension will reload automatically.

## Configuration

1. Start acrawl in REPL mode: `acrawl`
2. Type `/extension` — this starts the bridge server and prints a 6-digit **pairing code** (valid for 3 minutes)
3. Click the acrawl Bridge icon. The popup shows which acrawl session is asking to pair, with a countdown
4. Type the code into the popup and click **Pair** — the badge turns green

Over MCP, call any acrawl browser tool; if the extension isn't connected the tool error includes the pairing code, and the popup names your MCP client.

The code is only ever shown on the acrawl side and typed into the extension, so another process that can reach the port cannot pair by itself. Five wrong codes cancel the request. Pairing is one-time: the extension keeps the token and reconnects on its own.

Once connected, acrawl persists `browser_backend: "extension"` in `~/.acrawl/settings.json`. On subsequent launches, the bridge server auto-starts and waits for the extension to reconnect — no need to type `/extension` again.

## Switching Back

To switch back to CloakBrowser mode: type `/cloakbrowser` in the acrawl REPL. This clears the `browser_backend` setting and restores the default headless behavior.

## How It Works

The extension communicates with acrawl over a local WebSocket connection:

1. acrawl starts a TCP server on `127.0.0.1:<port>`
2. The extension connects to `ws://127.0.0.1:<port>/bridge?token=<token>`
3. acrawl sends `{id, action, payload}` commands (navigate, click, screenshot, etc.)
4. The extension executes them via Chrome DevTools Protocol (CDP) and returns `{id, ok, result}`

All browser automation tools work through this bridge — the same tool surface as CloakBrowser.

## Troubleshooting

- **Badge stays red**: Check that acrawl is running. If `browser_backend` is set to `"extension"` in settings, the server starts automatically on launch.
- **Connected before but not now (stale pairing)**: if acrawl's config was reset or you switched `ACRAWL_CONFIG_HOME`, the saved token no longer matches. Run `/extension` and enter the new pairing code in the popup.
- **Port in use**: Change the port in `~/.acrawl/settings.json` (`extension_bridge_port` field).
- **Extension disconnects frequently**: The extension uses exponential backoff reconnection (1s → 30s). Check if another process is competing for the port.

## Permissions

See [PRIVACY.md](PRIVACY.md) for a detailed explanation of each permission the extension requires.
