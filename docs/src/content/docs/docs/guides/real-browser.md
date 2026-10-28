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
| Tasks where you want to watch and step in | | ✅ |

## How it works

1. `/extension` starts a small server on `127.0.0.1`, port 19876 by default. It listens only on your machine, never on the network.
2. The extension, running in your browser, connects to it over a local WebSocket and reconnects on its own, retrying every couple of seconds, if acrawl restarts.
3. acrawl sends commands such as "navigate", "click" or "read the network log". The extension carries them out through the browser's DevTools Protocol and sends the results back. Tabs it opens for acrawl are kept together in an orange tab group, so you can see what the agent is working on.

The server accepts a connection only when the browser identifies it as coming from an extension. Ordinary web pages can also try to reach `127.0.0.1`, and they are refused, so a site you visit can't take control of acrawl or your browser through the bridge.

## What the extension can and can't see

- It drives the tabs acrawl works in and reads what those pages show, with your cookies and logins, the same as you would.
- It does nothing until acrawl connects, and it talks only to acrawl on your own machine.
- Whatever the agent reads goes to the LLM provider you configured, as with any acrawl run. Keep that in mind on pages with private data, or use a [local model](/AgenticCrawler/docs/providers/#local-and-self-hosted-models).

The full details are in the extension's [privacy policy](https://github.com/Mingye-Lu/AgenticCrawler/blob/main/extension/PRIVACY.md).

## Switch back to the headless browser

In the terminal UI, run `/cloakbrowser`. `/extension stop` stops the bridge too. From the shell or for the MCP server:

```bash
acrawl config unset browser_backend
```

## Troubleshooting

| Symptom | Fix |
|---|---|
| The extension badge stays red | acrawl isn't running the bridge. Run `/extension`, or set `browser_backend` to `extension` so it starts on launch. |
| It never connects | The port in the extension's options page must match `extension_bridge_port` in acrawl's settings (default 19876). |
| "Port in use" | Another program has the port. Pick another one with `acrawl config set extension_bridge_port 19877` and set the same port in the extension's options page. |
| The MCP server says another process owns the bridge | One acrawl process can hold the bridge at a time. Close the other terminal session, or let it finish, then retry. |
| An "errors" badge after a browser update | Click the extension icon; it reloads itself. |
| A tool says the extension isn't connected | Extension mode never falls back to the headless browser. Open your browser, check the badge, and retry. |
