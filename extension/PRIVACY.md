# Privacy Policy for acrawl Bridge

The acrawl Bridge extension connects your browser to a locally running acrawl process.

## Data Handling

- **No external network calls**: The extension communicates ONLY with `http://127.0.0.1` (localhost).
- **No analytics**: No usage data, telemetry, or crash reports are sent anywhere.
- **No cloud storage**: Settings (port, token) are stored in `chrome.storage.local` on your device only.
- **No user data collection**: The extension does not collect, store, or transmit personal information.

## Browser Access

The extension uses the `chrome.debugger` API to enable AI-driven automation of browser tabs that acrawl creates. It never accesses tabs created by the user.

## Contact

For questions, see the [acrawl repository](https://github.com/Mingye-Lu/AgenticCrawler).
