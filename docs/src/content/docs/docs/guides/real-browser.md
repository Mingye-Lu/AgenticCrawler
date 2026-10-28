---
title: "Use your real, logged-in browser"
description: "Drive your own logged-in Chrome, Edge or Brave with the acrawl Bridge extension, for sites behind a login or strict bot detection."
---

**Short answer:** install the acrawl Bridge extension and run `/extension`. acrawl then drives your own browser, with your sessions, cookies and extensions, instead of a headless one. Use it for sites behind a login or with strict bot detection.

## Set it up

1. Install **acrawl Bridge** from the [Chrome Web Store](https://chromewebstore.google.com/detail/acrawl-bridge/nbkpmfofkdjcgkncmpicbdkjmddllcep). The same listing works in Edge, Brave, Arc, Vivaldi and Opera.
2. In the acrawl terminal UI, run `/extension`. It starts the bridge, and the extension connects on its own within a couple of seconds; its badge turns green.
3. That's it. There's no pairing code or token: installing the extension is the permission for acrawl to drive your browser, and the bridge only accepts connections from a browser extension. acrawl remembers extension mode, so later sessions reconnect without running `/extension` again.

To install an unreleased build without the store, see the [extension README](https://github.com/Mingye-Lu/AgenticCrawler/blob/main/extension/README.md#manual-install-from-release).

## Use it from an MCP client

```bash
acrawl config set browser_backend extension
```

MCP browser tools now go through your browser. If the extension isn't connected, the tool call returns a clear error rather than quietly falling back to the headless browser, since the point of extension mode is your signed-in session. Switch back with:

```bash
acrawl config unset browser_backend
```

The extension serves one acrawl process at a time. If a terminal session already owns the bridge, the MCP server tells you which one.

## When to use it

| Situation | Headless (default) | Your browser |
|---|---|---|
| Public pages, batch jobs, CI | ✅ | |
| Content behind a login | | ✅ |
| CAPTCHAs or aggressive bot detection | | ✅ |
| Sites that check real mouse movement | | ✅ |
