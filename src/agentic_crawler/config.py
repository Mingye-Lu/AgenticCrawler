from __future__ import annotations

from pydantic import Field
from pydantic_settings import BaseSettings, SettingsConfigDict


class Settings(BaseSettings):
    model_config = SettingsConfigDict(env_file=".env", env_file_encoding="utf-8", extra="ignore")

    # LLM provider
    llm_provider: str = Field(default="claude", description="LLM provider: claude, openai, codex")
    anthropic_api_key: str = Field(default="", description="Anthropic API key")
    openai_api_key: str = Field(default="", description="OpenAI API key")
    openai_auth_method: str = Field(
        default="api_key", description="OpenAI auth method: api_key, oauth"
    )
    claude_model: str = Field(default="claude-sonnet-4-20250514", description="Claude model ID")
    openai_model: str = Field(default="gpt-4o", description="OpenAI model ID")
    codex_model: str = Field(default="codex-mini-latest", description="OpenAI Codex model ID")

    # Agent
    max_steps: int = Field(default=50, description="Maximum agent loop iterations")
    temperature: float = Field(default=0.0, description="LLM temperature")

    # Browser
    headless: bool = Field(default=True, description="Run browser in headless mode")
    browser_timeout: int = Field(default=30000, description="Browser action timeout (ms)")

    # Output
    output_format: str = Field(default="json", description="Output format: json, csv, stdout")
    output_file: str | None = Field(default=None, description="Output file path")
    workspace_dir: str = Field(default="workspace", description="Directory for saved files")

    # Fork limits
    max_concurrent_per_parent: int = Field(
        default=5, description="Max concurrent subagents per parent"
    )
    max_fork_depth: int = Field(default=3, description="Max fork recursion depth")
    max_total_agents: int = Field(default=10, description="Max total agents in fork tree")
    fork_child_max_steps: int = Field(default=15, description="Max steps for forked child agents")
    fork_wait_timeout: int = Field(
        default=60, description="Seconds to wait for children at done time"
    )


def get_settings(**overrides: object) -> Settings:
    return Settings(**overrides)  # type: ignore[arg-type]
