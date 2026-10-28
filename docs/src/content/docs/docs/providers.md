---
title: "LLM providers"
description: "acrawl works with 25 LLM providers, including Anthropic, OpenAI, Gemini, Bedrock and Azure, plus any OpenAI-compatible endpoint such as Ollama."
head:
  - tag: title
    content: "acrawl LLM providers: Anthropic, OpenAI, Gemini, Ollama and 21 more"
  - tag: meta
    attrs:
      property: og:title
      content: "acrawl LLM providers: Anthropic, OpenAI, Gemini, Ollama and 21 more"
---

**Short answer:** acrawl talks to 25 LLM providers directly, with no LiteLLM or proxy in between. Run `acrawl auth <provider>` once, and pick a model in `provider/model-id` form. Any server that speaks the OpenAI chat completions API, such as Ollama, LM Studio or vLLM, works through the `other` provider.

You only need a provider for the autonomous agent: the terminal UI, `acrawl prompt` and the MCP server's `run_goal` tool. The MCP server's direct browser and script tools, and `acrawl run`, make no LLM calls.

## Supported providers

| Category | Provider | `acrawl auth` id | Auth | Environment variable |
|---|---|---|---|---|
| Popular | Anthropic | `anthropic` | API key | `ANTHROPIC_API_KEY` |
| | OpenAI | `openai` | API key | `OPENAI_API_KEY` |
| | Google Gemini | `google` | API key | `GEMINI_API_KEY` |
| | DeepSeek | `deepseek` | API key | `DEEPSEEK_API_KEY` |
| Enterprise | Amazon Bedrock | `amazon-bedrock` | AWS SigV4 | `AWS_ACCESS_KEY_ID` |
| | Azure OpenAI | `azure` | Azure API key | `AZURE_OPENAI_API_KEY` |
| | Google Vertex AI | `vertex` | GCP service account | `GOOGLE_APPLICATION_CREDENTIALS` |
| | GitHub Copilot | `copilot` | Device OAuth | none |
| | SAP AI Core | `sap` | API key | `SAP_AI_CORE_API_KEY` |
| | GitLab Duo | `gitlab` | GitLab token | `GITLAB_TOKEN` |
| Open-model hosting | Groq | `groq` | API key | `GROQ_API_KEY` |
| | Cerebras | `cerebras` | API key | `CEREBRAS_API_KEY` |
| | DeepInfra | `deepinfra` | API key | `DEEPINFRA_API_KEY` |
| | Together AI | `togetherai` | API key | `TOGETHER_API_KEY` |
| | Mistral AI | `mistral` | API key | `MISTRAL_API_KEY` |
| Specialized | Perplexity | `perplexity` | API key | `PERPLEXITY_API_KEY` |
| | xAI (Grok) | `xai` | API key | `XAI_API_KEY` |
| | Cohere | `cohere` | API key | `COHERE_API_KEY` |
| | Alibaba (DashScope) | `alibaba` | API key | `DASHSCOPE_API_KEY` |
| Gateways | OpenRouter | `openrouter` | API key | `OPENROUTER_API_KEY` |
| | Vercel AI Gateway | `vercel` | API key | `VERCEL_API_KEY` |
| | Cloudflare Workers AI | `cloudflare` | API token | `CLOUDFLARE_API_TOKEN` |
| | Cloudflare AI Gateway | `cloudflare-gateway` | API token | `CLOUDFLARE_API_TOKEN` |
| Other | Venice AI | `venice` | API key | `VENICE_API_KEY` |
| | Any OpenAI-compatible server | `other` | Optional API key | none |

Environment variables are a fallback: acrawl reads them only when `credentials.json` has no entry for that provider. `acrawl auth list` prints this table from your installed version.

## Set up a provider

Interactively, in a terminal:

```bash
acrawl auth anthropic
```

Or without prompts, which suits CI and coding agents:

```bash
acrawl auth anthropic --api-key "sk-ant-..."
acrawl config set model anthropic/claude-sonnet-4-6
acrawl auth status --check anthropic   # exit 0 if ready, 3 if not
```

`--api-key` is visible in your shell history and process list. On a shared machine, prefer the interactive prompt or the provider's environment variable.

Credentials are saved to `~/.acrawl/credentials.json`. Set `ACRAWL_CONFIG_HOME` to keep them somewhere else. `acrawl auth status --json` prints what is configured, with secrets masked.

## Choose a model

Models are always written as `provider/model-id`:

```text
anthropic/claude-sonnet-4-6
openai/gpt-4o
amazon-bedrock/anthropic.claude-sonnet-4-6-20250514-v1:0
other/llama3.2
```

Set the default with `acrawl config set model <model>`, override it for one run with `--model`, or switch inside the terminal UI with `/model`, which opens a searchable list grouped by provider.

Which model works best? The agent reads pages, decides what to click and returns structured data, so it rewards models that are strong at tool use. Frontier models from Anthropic, OpenAI and Google are the safe choice for multi-step tasks; smaller open models are fine for single-page extraction. For repeated jobs, save the crawl as a [deterministic script](/AgenticCrawler/docs/guides/deterministic-scripts/) and replay it with no model at all.

## Local and self-hosted models

Point the `other` provider at any OpenAI-compatible endpoint:

```bash
acrawl auth other --base-url http://localhost:11434/v1
```

Leave the API key empty if your server doesn't need one.

| Server | Base URL | Model string |
|---|---|---|
| Ollama | `http://localhost:11434/v1` | `other/llama3.2` |
| LM Studio | `http://localhost:1234/v1` | `other/local-model` |
| vLLM | `http://localhost:8000/v1` | `other/meta-llama/Llama-3.1-8B` |
| Any OpenAI-compatible API | your endpoint | `other/<model-id>` |

The model must support tool calling. Without it, the agent can't use its browser tools.

## Providers with their own sign-in

**GitHub Copilot.** `acrawl auth copilot` prints a code and opens `https://github.com/login/device`. Enter the code and approve; acrawl then swaps your GitHub token for a short-lived Copilot token and stores it. No API key is involved.

**Amazon Bedrock.** `acrawl auth amazon-bedrock` asks for an access key ID, secret key and region (default `us-east-1`), and signs requests with AWS SigV4. Non-interactively: `--access-key`, `--secret-key`, `--region`.

**Azure OpenAI.** `acrawl auth azure` asks for the resource name, deployment name and API key. Non-interactively: `--resource-name`, `--deployment-name`, `--api-key`.

**Google Vertex AI.** Uses a service account. Pass `--gcp-project` and `--gcp-region`, and point `GOOGLE_APPLICATION_CREDENTIALS` at the key file.

**GitLab Duo.** `acrawl auth gitlab` takes a personal access token. GitLab Duo doesn't support tool calling, so it can chat but can't drive the browser.

## Related

- [Configuration](/AgenticCrawler/docs/configuration/): every setting in `credentials.json` and `settings.json`
- [Quick start](/AgenticCrawler/docs/quickstart/): run a first crawl once a provider is set up
