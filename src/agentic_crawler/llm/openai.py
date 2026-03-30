from __future__ import annotations

import base64
import json
from collections.abc import Callable
from pathlib import Path
from typing import Any

import httpx
import openai

from agentic_crawler.llm.base import LLMResponse, ToolCall

CODEX_BASE_URL = "https://chatgpt.com/backend-api"
CODEX_RESPONSES_PATH = "/codex/responses"
JWT_CLAIM_PATH = "https://api.openai.com/auth"


def _extract_account_id(access_token: str) -> str | None:
    try:
        payload_b64 = access_token.split(".")[1]
        payload_b64 += "=" * (4 - len(payload_b64) % 4)
        payload = json.loads(base64.urlsafe_b64decode(payload_b64))
        return payload.get(JWT_CLAIM_PATH, {}).get("chatgpt_account_id")
    except Exception:
        return None


class OpenAIProvider:
    def __init__(
        self,
        api_key: str | None = None,
        model: str = "gpt-4o",
        *,
        use_oauth: bool = False,
    ) -> None:
        self.model = model
        self._use_oauth = use_oauth
        self._http_client: httpx.AsyncClient | None = None
        if use_oauth:
            from agentic_crawler.llm.oauth import load_tokens

            tokens = load_tokens()
            if tokens is None:
                raise RuntimeError("No OAuth tokens found. Run `agentic-crawler login` first.")
            self._oauth_token_path = Path.home() / ".codex" / "auth.json"
            self._access_token = tokens.access_token
            self.client = openai.AsyncOpenAI(api_key="unused")
        else:
            self._access_token = ""
            self.client = openai.AsyncOpenAI(api_key=api_key)

    async def _refresh_oauth_if_needed(self) -> None:
        if not self._use_oauth:
            return
        from agentic_crawler.llm.oauth import ensure_valid_tokens

        tokens = await ensure_valid_tokens(self._oauth_token_path)
        self._access_token = tokens.access_token

    async def _get_http_client(self) -> httpx.AsyncClient:
        if self._http_client is None or self._http_client.is_closed:
            self._http_client = httpx.AsyncClient(timeout=120.0)
        return self._http_client

    async def complete(
        self,
        messages: list[dict[str, Any]],
        tools: list[dict[str, Any]] | None = None,
        temperature: float = 0.0,
        on_thinking: Callable[[str], None] | None = None,
    ) -> LLMResponse:
        await self._refresh_oauth_if_needed()

        if self._use_oauth:
            return await self._complete_codex_api(messages, tools, temperature, on_thinking)
        return await self._complete_chat_api(messages, tools, temperature, on_thinking)

    async def _complete_chat_api(
        self,
        messages: list[dict[str, Any]],
        tools: list[dict[str, Any]] | None,
        temperature: float,
        on_thinking: Callable[[str], None] | None = None,
    ) -> LLMResponse:
        kwargs: dict[str, Any] = {
            "model": self.model,
            "temperature": temperature,
            "messages": messages,
            "stream": True,
            "stream_options": {"include_usage": True},
        }
        if tools:
            kwargs["tools"] = self._convert_tools_chat(tools)

        stream = await self.client.chat.completions.create(**kwargs)

        text_parts: list[str] = []
        thinking_parts: list[str] = []
        tool_call_buffers: dict[int, dict[str, str]] = {}
        usage_data: dict[str, int] = {"input_tokens": 0, "output_tokens": 0}

        async for chunk in stream:
            if not chunk.choices:
                # Final chunk with usage only
                if chunk.usage:
                    usage_data["input_tokens"] = chunk.usage.prompt_tokens
                    usage_data["output_tokens"] = chunk.usage.completion_tokens
                continue

            delta = chunk.choices[0].delta

            # Reasoning content (o-series models)
            reasoning = getattr(delta, "reasoning_content", None)
            if reasoning:
                thinking_parts.append(reasoning)
                if on_thinking:
                    on_thinking(reasoning)

            # Text content
            if delta.content:
                text_parts.append(delta.content)

            # Tool call deltas
            if delta.tool_calls:
                for tc_delta in delta.tool_calls:
                    idx = tc_delta.index
                    if idx not in tool_call_buffers:
                        tool_call_buffers[idx] = {
                            "id": tc_delta.id or "",
                            "name": tc_delta.function.name if tc_delta.function and tc_delta.function.name else "",
                            "arguments": "",
                        }
                    buf = tool_call_buffers[idx]
                    if tc_delta.id:
                        buf["id"] = tc_delta.id
                    if tc_delta.function and tc_delta.function.name:
                        buf["name"] = tc_delta.function.name
                    if tc_delta.function and tc_delta.function.arguments:
                        buf["arguments"] += tc_delta.function.arguments

            # Usage on final chunk
            if chunk.usage:
                usage_data["input_tokens"] = chunk.usage.prompt_tokens
                usage_data["output_tokens"] = chunk.usage.completion_tokens

        tool_calls: list[ToolCall] = []
        for idx in sorted(tool_call_buffers):
            buf = tool_call_buffers[idx]
            tool_calls.append(
                ToolCall(
                    id=buf["id"],
                    name=buf["name"],
                    arguments=json.loads(buf["arguments"]) if buf["arguments"] else {},
                )
            )

        full_thinking = "".join(thinking_parts) if thinking_parts else None

        return LLMResponse(
            text="".join(text_parts) if text_parts else None,
            tool_calls=tool_calls,
            usage=usage_data,
            thinking=full_thinking,
        )

    async def _complete_codex_api(
        self,
        messages: list[dict[str, Any]],
        tools: list[dict[str, Any]] | None,
        temperature: float,
        on_thinking: Callable[[str], None] | None = None,
    ) -> LLMResponse:
        """Route through chatgpt.com/backend-api/codex/responses.

        ChatGPT OAuth tokens are scoped to the ChatGPT backend, not the
        public api.openai.com endpoints.  The Codex Responses API is the
        only endpoint that accepts them.
        """
        account_id = _extract_account_id(self._access_token)

        instructions: str | None = None
        input_items: list[dict[str, Any]] = []

        for msg in messages:
            role = msg.get("role", "")
            content = msg.get("content", "")
            if role == "system":
                if instructions is None:
                    instructions = content
                else:
                    instructions += "\n\n" + content
            elif role == "assistant":
                if "tool_calls" in msg:
                    for tc in msg["tool_calls"]:
                        input_items.append(
                            {
                                "type": "function_call",
                                "call_id": tc["id"],
                                "name": tc["function"]["name"],
                                "arguments": tc["function"]["arguments"],
                            }
                        )
                elif content:
                    input_items.append({"role": "assistant", "content": content})
            elif role == "tool":
                input_items.append(
                    {
                        "type": "function_call_output",
                        "call_id": msg.get("tool_call_id", ""),
                        "output": content,
                    }
                )
            else:
                input_items.append({"role": "user", "content": content})

        body: dict[str, Any] = {
            "model": self.model,
            "input": input_items,
            "stream": True,
            "store": False,
        }
        if instructions:
            body["instructions"] = instructions
        if tools:
            body["tools"] = self._convert_tools_responses(tools)

        headers: dict[str, str] = {
            "Authorization": f"Bearer {self._access_token}",
            "Content-Type": "application/json",
            "OpenAI-Beta": "responses=experimental",
            "originator": "codex",
        }
        if account_id:
            headers["chatgpt-account-id"] = account_id

        headers["Accept"] = "text/event-stream"

        client = await self._get_http_client()
        url = f"{CODEX_BASE_URL}{CODEX_RESPONSES_PATH}"

        text_parts: list[str] = []
        thinking_parts: list[str] = []
        tool_calls: list[ToolCall] = []
        usage_data: dict[str, int] = {"input_tokens": 0, "output_tokens": 0}

        async with client.stream("POST", url, json=body, headers=headers) as resp:
            if resp.status_code != 200:
                error_body = await resp.aread()
                raise RuntimeError(f"Codex API {resp.status_code}: {error_body.decode()}")

            current_text = ""
            current_tool: dict[str, Any] | None = None

            async for line in resp.aiter_lines():
                if not line.startswith("data: "):
                    continue
                payload = line[6:]
                if payload == "[DONE]":
                    break

                event = json.loads(payload)
                event_type = event.get("type", "")

                if event_type == "response.output_text.delta":
                    current_text += event.get("delta", "")
                elif event_type == "response.output_text.done":
                    if current_text:
                        text_parts.append(current_text)
                    current_text = ""
                elif event_type == "response.reasoning_summary_text.delta":
                    chunk = event.get("delta", "")
                    thinking_parts.append(chunk)
                    if on_thinking:
                        on_thinking(chunk)
                elif event_type == "response.function_call_arguments.delta":
                    if current_tool is None:
                        current_tool = {
                            "call_id": event.get("call_id", ""),
                            "name": event.get("name", ""),
                            "arguments": "",
                        }
                    current_tool["arguments"] += event.get("delta", "")
                elif event_type == "response.function_call_arguments.done":
                    if current_tool:
                        tool_calls.append(
                            ToolCall(
                                id=current_tool["call_id"],
                                name=current_tool["name"],
                                arguments=json.loads(current_tool["arguments"]),
                            )
                        )
                        current_tool = None
                elif event_type == "response.output_item.added":
                    item = event.get("item", {})
                    if item.get("type") == "function_call":
                        current_tool = {
                            "call_id": item.get("call_id", ""),
                            "name": item.get("name", ""),
                            "arguments": "",
                        }
                elif event_type == "response.completed":
                    resp_obj = event.get("response", {})
                    u = resp_obj.get("usage", {})
                    usage_data["input_tokens"] = u.get("input_tokens", 0)
                    usage_data["output_tokens"] = u.get("output_tokens", 0)

            if current_text:
                text_parts.append(current_text)

        full_thinking = "".join(thinking_parts) if thinking_parts else None

        return LLMResponse(
            text="\n".join(text_parts) if text_parts else None,
            tool_calls=tool_calls,
            usage=usage_data,
            thinking=full_thinking,
        )

    def _convert_tools_chat(self, tools: list[dict[str, Any]]) -> list[dict[str, Any]]:
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

    def _convert_tools_responses(self, tools: list[dict[str, Any]]) -> list[dict[str, Any]]:
        responses_tools = []
        for tool in tools:
            responses_tools.append(
                {
                    "type": "function",
                    "name": tool["name"],
                    "description": tool.get("description", ""),
                    "parameters": tool.get("parameters", {"type": "object", "properties": {}}),
                }
            )
        return responses_tools

    async def close(self) -> None:
        if self._http_client and not self._http_client.is_closed:
            await self._http_client.aclose()
        await self.client.close()
