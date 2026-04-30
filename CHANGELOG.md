# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-04-29

### Added

- LLM-driven agent loop with a 19-tool toolbox (16 browser + 3 agent-control).
- 24 LLM providers: Anthropic, OpenAI, Google Gemini, AWS Bedrock, Azure OpenAI, Vertex AI, GitHub Copilot, SAP AI Core, GitLab Duo, Groq, Cerebras, DeepInfra, Together AI, Mistral, Perplexity, xAI, Cohere, Alibaba DashScope, OpenRouter, Vercel AI, Cloudflare Workers/Gateway, Venice AI, and any OpenAI-compatible endpoint.
- Smart fetch routing — HTTP-first with automatic escalation to headless Chromium when JS framework markers, auth redirects, or `<noscript>` bodies are detected.
- Sub-agent parallelism via `fork`/`wait_for_subagents`/`done` with configurable concurrency limits, fork depth, and step budgets.
- Interactive TUI REPL with markdown rendering, syntax highlighting, streaming output, model picker, and auth modal.
- 16 slash commands: `/help`, `/status`, `/model`, `/compact`, `/clear`, `/cost`, `/session`, `/export`, `/resume`, `/config`, `/auth`, `/headed`, `/headless`, `/debug`, `/version`, `/exit`.
- Session persistence with auto-save, resume, export to markdown, and auto-compaction.
- Permission model: `read-only`, `workspace-write`, `danger-full-access`.
- MCP (Model Context Protocol) server support with stdio, SSE, HTTP, and WebSocket transports.
- One-shot CLI mode via `acrawl prompt "..."`.
- Model aliases for quick switching (`sonnet`, `opus`, `haiku`, `4o`, `o3`, etc.) and provider-prefixed names.
- Reasoning effort cycling (`Ctrl+T`) for reasoning models.
- OAuth PKCE flow for Codex and GitHub Copilot, AWS SigV4 for Bedrock, GCP auth for Vertex AI.
- Structured output in JSON, CSV, or plain text.
- Credential management via `acrawl auth` with per-provider configuration.

[0.1.0]: https://github.com/Mingye-Lu/AgenticCrawler/releases/tag/v0.1.0
