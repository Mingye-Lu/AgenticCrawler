from __future__ import annotations

import json
from typing import Any

import anthropic

from agentic_crawler.llm.base import LLMResponse, ToolCall


class ClaudeProvider:
    def __init__(self, api_key: str, model: str = "claude-sonnet-4-20250514") -> None:
        self.model = model
        self.client = anthropic.AsyncAnthropic(api_key=api_key)

    async def complete(
        self,
        messages: list[dict[str, Any]],
        tools: list[dict[str, Any]] | None = None,
        temperature: float = 0.0,
    ) -> LLMResponse:
        # Separate system message from conversation
        system_text = ""
        conversation: list[dict[str, Any]] = []
        for msg in messages:
            if msg["role"] == "system":
                system_text = msg["content"]
            else:
                conversation.append(msg)

        kwargs: dict[str, Any] = {
            "model": self.model,
            "max_tokens": 4096,
            "temperature": temperature,
            "messages": conversation,
        }
        if system_text:
            kwargs["system"] = system_text
        if tools:
            kwargs["tools"] = self._convert_tools(tools)

        response = await self.client.messages.create(**kwargs)

        # Parse response
        text_parts: list[str] = []
        tool_calls: list[ToolCall] = []

        for block in response.content:
            if block.type == "text":
                text_parts.append(block.text)
            elif block.type == "tool_use":
                tool_calls.append(
                    ToolCall(
                        id=block.id,
                        name=block.name,
                        arguments=dict(block.input) if isinstance(block.input, dict) else {},
                    )
                )

        return LLMResponse(
            text="\n".join(text_parts) if text_parts else None,
            tool_calls=tool_calls,
            usage={
                "input_tokens": response.usage.input_tokens,
                "output_tokens": response.usage.output_tokens,
            },
        )

    def _convert_tools(self, tools: list[dict[str, Any]]) -> list[dict[str, Any]]:
        """Convert generic tool schema to Anthropic format."""
        anthropic_tools = []
        for tool in tools:
            anthropic_tools.append(
                {
                    "name": tool["name"],
                    "description": tool.get("description", ""),
                    "input_schema": tool.get("parameters", {"type": "object", "properties": {}}),
                }
            )
        return anthropic_tools

    async def close(self) -> None:
        await self.client.close()
