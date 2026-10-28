---
title: "About acrawl"
description: "acrawl is an MIT-licensed browser agent built in the open by Mingye Lu since March 2026: why it exists, how it is built, and who is behind it."
head:
  - tag: title
    content: "About acrawl: an open-source browser agent written in Rust"
  - tag: meta
    attrs:
      property: og:title
      content: "About acrawl: an open-source browser agent written in Rust"
---

acrawl is an open-source browser agent and MCP server, shipped as a single Rust binary. You describe a web task in plain English; it navigates, clicks, fills forms, watches the network and returns structured data. It is MIT-licensed, runs on your own machine and uses your own LLM key.

## Why it exists

Most browser agents stop at navigating and clicking, and most need a Python or Node runtime, a cloud account, or both. acrawl started from three convictions:

- **An agent should see what a developer sees.** Network requests, console errors, performance and accessibility are first-class tools, not an afterthought. That makes acrawl useful for debugging a page, not only for scraping it.
- **Pay for intelligence once.** Use the model to work out a crawl, then replay it as a [deterministic script](/AgenticCrawler/docs/guides/deterministic-scripts/) with no model calls at all.
- **Nothing to install but the binary.** One executable for Linux, macOS and Windows, with a stealth browser fetched on first use. No Docker, no virtualenv, no hosted service.

## Who builds it

acrawl is created and maintained by [Mingye Lu](https://github.com/Mingye-Lu), with code from [outside contributors](https://github.com/Mingye-Lu/AgenticCrawler/graphs/contributors). Development happens in public on [GitHub](https://github.com/Mingye-Lu/AgenticCrawler): every change goes through a pull request and continuous integration, the test suite runs to more than 1,100 tests, and each release is built for five platforms.

| Milestone | Date |
|---|---|
| First commit | March 30, 2026 |
| First release (0.1.0) | April 29, 2026 |
| Real-browser extension bridge (0.5.0) | May 21, 2026 |
| Built-in MCP server (0.6.0) | May 22, 2026 |
| This website | October 2026 |

Every release is listed with its notes in the [changelog](https://github.com/Mingye-Lu/AgenticCrawler/blob/main/CHANGELOG.md).

## How it is built

- **Rust**, in a workspace of 11 crates: provider clients, browser bridges, the agent and its tools, the conversation runtime, the terminal UI and the MCP server. See [How acrawl works](/AgenticCrawler/docs/how-it-works/).
- **CloakBrowser** for stealth headless browsing, and the **acrawl Bridge** extension for driving your own Chrome, Edge or Brave.
- **axe-core** for accessibility audits and **Ratatui** for the terminal UI.
- Several optional [optimizations](/AgenticCrawler/docs/configuration/#optimizations-optimization) adapt ideas published by other open-source agents, including browser-use, Stagehand, crawl4ai, Skyvern, Spider and nanobrowser.

## What acrawl sends, and where

acrawl has no telemetry and no account. It talks to:

- **Your LLM provider**, with the page content the agent needs, when you run the agent.
- **The sites you ask it to visit.**
- **GitHub**, to check for a newer release on startup and to download one when you run `acrawl update`.
- **TypeSafe**, only if you turn on the optional `semantic_prune` setting and give it a key.

The browser extension's data handling is described in its [privacy policy](https://github.com/Mingye-Lu/AgenticCrawler/blob/main/extension/PRIVACY.md).

## Get involved

- **Report a bug or ask for a feature:** [open an issue](https://github.com/Mingye-Lu/AgenticCrawler/issues).
- **Contribute code:** start with [CONTRIBUTING.md](https://github.com/Mingye-Lu/AgenticCrawler/blob/main/CONTRIBUTING.md).
- **Report a security problem privately:** see [SECURITY.md](https://github.com/Mingye-Lu/AgenticCrawler/blob/main/SECURITY.md).

acrawl is released under the [MIT License](https://github.com/Mingye-Lu/AgenticCrawler/blob/main/LICENSE).
