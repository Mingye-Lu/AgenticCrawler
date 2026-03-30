from __future__ import annotations

from agentic_crawler.config import Settings
from agentic_crawler.llm.base import LLMProvider


def get_provider(settings: Settings) -> LLMProvider:
    """Create an LLM provider from settings."""
    name = settings.llm_provider.lower()

    if name == "claude":
        from agentic_crawler.llm.claude import ClaudeProvider

        if not settings.anthropic_api_key:
            raise ValueError("ANTHROPIC_API_KEY is required for Claude provider")
        return ClaudeProvider(api_key=settings.anthropic_api_key, model=settings.claude_model)

    if name == "openai":
        from agentic_crawler.llm.openai import OpenAIProvider

        if settings.openai_auth_method == "oauth":
            return OpenAIProvider(model=settings.openai_model, use_oauth=True)
        if not settings.openai_api_key:
            raise ValueError("OPENAI_API_KEY is required for OpenAI provider")
        return OpenAIProvider(api_key=settings.openai_api_key, model=settings.openai_model)

    if name == "codex":
        from agentic_crawler.llm.openai import OpenAIProvider

        return OpenAIProvider(model=settings.codex_model, use_oauth=True)

    raise ValueError(f"Unknown LLM provider: {name}. Supported: claude, openai, codex")
