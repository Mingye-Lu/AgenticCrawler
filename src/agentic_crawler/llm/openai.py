from __future__ import annotations

import base64
import json
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
    ) -> LLMResponse:
        await self._refresh_oauth_if_needed()

        if self._use_oauth:
            return await self._complete_codex_api(messages, tools, temperature)
        return await self._complete_chat_api(messages, tools, temperature)

    async def _complete_chat_api(
        self,
        messages: list[dict[str, Any]],
        tools: list[dict[str, Any]] | None,
        temperature: float,
    ) -> LLMResponse:
        kwargs: dict[str, Any] = {
            "model": self.model,
            "temperature": temperature,
            "messages": messages,
        }
        if tools:
            kwargs["tools"] = self._convert_tools_chat(tools)

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

    async def _complete_codex_api(
        self,
        messages: list[dict[str, Any]],
        tools: list[dict[str, Any]] | None,
        temperature: float,
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
                input_items.append({"role": "assistant", "content": content})
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

        return LLMResponse(
            text="\n".join(text_parts) if text_parts else None,
            tool_calls=tool_calls,
            usage=usage_data,
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
