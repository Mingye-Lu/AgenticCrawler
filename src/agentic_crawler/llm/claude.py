from __future__ import annotations

import json
from collections.abc import Callable
from typing import Any

import anthropic

from agentic_crawler.llm.base import LLMResponse, ToolCall

THINKING_BUDGET_TOKENS = 10_000


class ClaudeProvider:
    def __init__(self, model: str = "claude-sonnet-4-20250514", api_key: str = "") -> None:
        self.model = model
        self.client = anthropic.AsyncAnthropic(api_key=api_key or None)

    async def complete(
        self,
        messages: list[dict[str, Any]],
        tools: list[dict[str, Any]] | None = None,
        temperature: float = 0.0,
        on_thinking: Callable[[str], None] | None = None,
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
            "max_tokens": 16_000,
            "messages": conversation,
            "thinking": {"type": "enabled", "budget_tokens": THINKING_BUDGET_TOKENS},
        }
        # Extended thinking requires temperature not be set
        if system_text:
            kwargs["system"] = system_text
        if tools:
            kwargs["tools"] = self._convert_tools(tools)

        # Use streaming API
        text_parts: list[str] = []
        thinking_parts: list[str] = []
        tool_calls: list[ToolCall] = []

        async with self.client.messages.stream(**kwargs) as stream:
            async for event in stream:
                if (
                    event.type == "content_block_delta"
                    and event.delta.type == "thinking_delta"
                ):
                    chunk = event.delta.thinking
                    thinking_parts.append(chunk)
                    if on_thinking:
                        on_thinking(chunk)

            final = await stream.get_final_message()

        # Parse the final message for complete blocks
        for block in final.content:
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

        full_thinking = "".join(thinking_parts) if thinking_parts else None

        return LLMResponse(
            text="\n".join(text_parts) if text_parts else None,
            tool_calls=tool_calls,
            usage={
                "input_tokens": final.usage.input_tokens,
                "output_tokens": final.usage.output_tokens,
            },
            thinking=full_thinking,
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
