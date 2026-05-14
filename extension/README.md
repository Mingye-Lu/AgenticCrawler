# acrawl Bridge Extension

Connects [acrawl](https://github.com/Mingye-Lu/AgenticCrawler) to your Chromium browser for AI-driven web automation.

## Installation

1. Install from the [Chrome Web Store](https://chrome.google.com/webstore) (link pending)
2. Or load unpacked: open `chrome://extensions`, enable Developer Mode, click "Load unpacked", select this `extension/` folder.

## Configuration

1. Start acrawl in REPL mode: `acrawl`
2. Type `/extension` — this starts the bridge server and displays a token
3. Open extension options (click the acrawl Bridge icon → Options)
4. Enter the port (default: `19876`) and paste the token
5. Click Save — the badge should turn green

## Switching Back

To switch back to CloakBrowser mode: type `/cloakbrowser` in the acrawl REPL.

## Troubleshooting

- **Badge stays red**: Check that acrawl is running and you've typed `/extension`
- **Token mismatch**: The token is shown once when you type `/extension`. Re-type `/extension` for a new server start.
- **Port in use**: Change the port in `~/.acrawl/settings.json` (`extension_bridge_port` field)
