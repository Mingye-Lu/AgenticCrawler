# acrawl (AgenticCrawler)

**acrawl is an open-source browser agent and MCP server that ships as a single Rust binary.** You describe a goal in plain English, and it navigates, clicks, fills forms and returns structured data. It can also observe the page the way a developer does: network requests, console errors, performance, cookies, storage and accessibility are all tools the agent can call.

<video class="promo-video" controls preload="metadata" playsinline poster="media/acrawl-promo-poster.jpg">
  <source src="media/acrawl-promo.mp4" type="video/mp4">
  Your browser can't play this video. <a href="media/acrawl-promo.mp4">Download the acrawl demo (MP4, 52 s)</a>.
</video>

```bash
acrawl prompt 'find the best 14-inch laptop under $1,500'
```

## What makes it different

- **One binary.** `acrawl` is a compiled Rust executable. There is no Python environment or Docker image to manage. Browser automation uses a bundled stealth browser (CloakBrowser) driven through a small Node.js bridge, so browser features need Node.js 20+.
- **Acts and observes.** Beyond navigate and click, the agent can list and inspect network requests, deduplicate console logs, read WebSocket frames, measure TTFB and resource sizes, audit cookies and storage, measure JS/CSS coverage, run axe-core WCAG audits, and block or mock requests.
- **AI once, then a script.** Repetitive work (paging through 50 result pages, say) can run as a deterministic JSON script with loops, conditionals and parallel branches, and no LLM calls per step. The agent handles the parts that need judgment.
- **Smart fetching.** Static pages are fetched over plain HTTP. When acrawl detects a JavaScript framework, an auth redirect or an empty SPA shell, it replays the request in a headless browser automatically.
- **Sub-agents.** The agent can fork child agents onto separate tabs, each with its own step budget. A URL-claiming registry stops siblings from crawling the same page twice.
- **MCP both ways.** acrawl can use tools from other MCP servers, and `acrawl mcp` exposes 40 of its own tools to Claude Code, Cursor, VS Code, Zed and other MCP clients.
- **25 LLM providers.** Anthropic, OpenAI, Google Gemini, DeepSeek, AWS Bedrock, Azure OpenAI, Vertex AI, GitHub Copilot, Groq, Mistral, xAI, OpenRouter and more, plus any OpenAI-compatible endpoint.

![acrawl architecture: the TUI, one-shot prompt and MCP entry points feed a runtime turn loop that streams from an LLM provider and runs tools, which fetch over HTTP, escalate to CloakBrowser, or drive the user's Chrome through an extension bridge.](media/architecture.svg)

## Key facts

| | |
|---|---|
| License | MIT |
| Language | Rust |
| Platforms | Linux and macOS (x64, ARM64), Windows (x64) |
| Agent tools | 43 (navigation, interaction, extraction, DevTools, sub-agents, scripts) |
| MCP server tools | 40 (32 browser and DevTools, 7 script, 1 autonomous `run_goal`), stdio transport |
| LLM providers | 25, plus any OpenAI-compatible endpoint |
| Source | [github.com/Mingye-Lu/AgenticCrawler](https://github.com/Mingye-Lu/AgenticCrawler) |

## Next steps

- [Install acrawl](install.md)
- [Run your first crawl](quickstart.md)
- [Add acrawl to Claude Code, Cursor or VS Code](mcp-server.md)
- [See how it compares with browser-use, Playwright MCP and Firecrawl](comparisons/overview.md)

<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "SoftwareApplication",
      "name": "acrawl",
      "alternateName": "AgenticCrawler",
      "description": "Open-source browser agent and MCP server in a single Rust binary. Describe a goal in plain English and get structured data back, with full DevTools observability and 25 LLM providers.",
      "applicationCategory": "DeveloperApplication",
      "operatingSystem": "Linux, macOS, Windows",
      "license": "https://opensource.org/licenses/MIT",
      "url": "https://mingye-lu.github.io/AgenticCrawler/",
      "codeRepository": "https://github.com/Mingye-Lu/AgenticCrawler",
      "programmingLanguage": "Rust",
      "author": { "@type": "Person", "name": "Mingye Lu", "url": "https://github.com/Mingye-Lu" },
      "offers": { "@type": "Offer", "price": "0", "priceCurrency": "USD" }
    },
    {
      "@type": "VideoObject",
      "name": "acrawl demo: describe it, acrawl it",
      "description": "acrawl plans a shopping task in plain English, turns a 50-page search into a script with zero LLM calls, prunes pages to what matters, and verifies the checkout.",
      "thumbnailUrl": "https://mingye-lu.github.io/AgenticCrawler/media/acrawl-promo-poster.jpg",
      "contentUrl": "https://mingye-lu.github.io/AgenticCrawler/media/acrawl-promo.mp4",
      "uploadDate": "2026-10-13",
      "duration": "PT52S"
    }
  ]
}
</script>
