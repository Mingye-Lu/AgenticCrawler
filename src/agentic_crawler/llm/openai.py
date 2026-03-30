from __future__ import annotations

import json
from typing import Any

import openai

from agentic_crawler.llm.base import LLMResponse, ToolCall


class OpenAIProvider:
    def __init__(self, api_key: str, model: str = "gpt-4o") -> None:
        self.model = model
        self.client = openai.AsyncOpenAI(api_key=api_key)

    async def complete(
        self,
        messages: list[dict[str, Any]],
        tools: list[dict[str, Any]] | None = None,
        temperature: float = 0.0,
    ) -> LLMResponse:
        kwargs: dict[str, Any] = {
            "model": self.model,
            "temperature": temperature,
            "messages": messages,
        }
        if tools:
            kwargs["tools"] = self._convert_tools(tools)

        response = await self.client.chat.completions.create(**kwargs)
        choice = response.choices[0]
        message = choice.message

        tool_calls: list[ToolCall] = []
        if message.tool_calls:
            for tc in message.tool_calls:
                tool_calls.append(
                    ToolCall(
                        id=tc.id,
                        name=tc.function.name,
                        arguments=json.loads(tc.function.arguments),
                    )
                )

        return LLMResponse(
            text=message.content,
            tool_calls=tool_calls,
            usage={
                "input_tokens": response.usage.prompt_tokens if response.usage else 0,
                "output_tokens": response.usage.completion_tokens if response.usage else 0,
            },
        )

    def _convert_tools(self, tools: list[dict[str, Any]]) -> list[dict[str, Any]]:
        """Convert generic tool schema to OpenAI function calling format."""
        openai_tools = []
        for tool in tools:
            openai_tools.append(
                {
                    "type": "function",
                    "function": {
                        "name": tool["name"],
                        "description": tool.get("description", ""),
                        "parameters": tool.get("parameters", {"type": "object", "properties": {}}),
                    },
                }
            )
        return openai_tools

    async def close(self) -> None:
        await self.client.close()
