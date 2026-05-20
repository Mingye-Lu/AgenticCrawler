# Privacy Policy for acrawl Bridge

The acrawl Bridge extension connects your browser to a locally running acrawl process for AI-driven web automation.

## Data Handling

- **Local communication only**: The extension connects to `http://127.0.0.1` (localhost) to communicate with the local acrawl process via WebSocket.
- **Web requests on behalf of acrawl**: When instructed by the local acrawl agent, the extension may fetch URLs (e.g., for file downloads) using your browser's session. These requests go directly to the target server — no data is routed through third parties.
- **No analytics**: No usage data, telemetry, or crash reports are sent anywhere.
- **No cloud storage**: Settings (port, token) are stored in `chrome.storage.local` on your device only.
- **No user data collection**: The extension does not independently collect, store, or transmit personal information.

## Permissions Explained

- **`debugger`**: Controls browser tabs via Chrome DevTools Protocol for automation (click, navigate, extract content).
- **`tabs`**: Manages tab lifecycle for the automation agent.
- **`host_permissions` (`<all_urls>`)**: Required to attach the debugger to any website the agent navigates to, and to fetch files on behalf of the agent using your authenticated session.
- **`storage`**: Stores connection settings (port, auth token) locally.
- **`alarms`**: Keepalive mechanism for the service worker.

## Browser Access

The extension uses the `chrome.debugger` API to enable AI-driven automation of browser tabs. It attaches to tabs created or navigated by the acrawl agent.

## Contact

For questions, see the [acrawl repository](https://github.com/Mingye-Lu/AgenticCrawler).
