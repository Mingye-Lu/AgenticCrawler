"""TDD tests for streaming LLM calls with extended thinking support."""

from __future__ import annotations

from typing import Any
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

from agentic_crawler.llm.base import LLMResponse, ToolCall


async def _async_iter(items: list[Any]) -> Any:
    for item in items:
        yield item


# ---------------------------------------------------------------------------
# 1. LLMResponse.thinking field
# ---------------------------------------------------------------------------


class TestLLMResponseThinking:
    def test_response_has_thinking_field(self) -> None:
        resp = LLMResponse(text="hello", thinking="I need to reason about this")
        assert resp.thinking == "I need to reason about this"

    def test_response_thinking_defaults_to_none(self) -> None:
        resp = LLMResponse(text="hello")
        assert resp.thinking is None

    def test_response_with_thinking_and_tool_calls(self) -> None:
        resp = LLMResponse(
            text=None,
            thinking="Let me decide which tool to use",
            tool_calls=[ToolCall(id="1", name="navigate", arguments={"url": "https://example.com"})],
        )
        assert resp.thinking is not None
        assert resp.has_tool_calls


# ---------------------------------------------------------------------------
# 2. Provider protocol accepts on_thinking callback
# ---------------------------------------------------------------------------


class TestProviderOnThinkingCallback:
    @pytest.mark.asyncio
    async def test_mock_provider_accepts_on_thinking(self) -> None:
        """The complete() method must accept an on_thinking keyword arg."""
        from tests.conftest import MockLLMProvider

        provider = MockLLMProvider(responses=[LLMResponse(text="ok")])
        chunks: list[str] = []
        result = await provider.complete(
            messages=[{"role": "user", "content": "hi"}],
            on_thinking=lambda chunk: chunks.append(chunk),
        )
        assert result.text == "ok"

    @pytest.mark.asyncio
    async def test_mock_provider_fires_thinking_callback(self) -> None:
        """When a response has thinking, the callback should receive it."""
        from tests.conftest import MockLLMProvider

        provider = MockLLMProvider(
            responses=[LLMResponse(text="answer", thinking="step by step reasoning")]
        )
        chunks: list[str] = []
        result = await provider.complete(
            messages=[{"role": "user", "content": "think about this"}],
            on_thinking=lambda chunk: chunks.append(chunk),
        )
        assert chunks == ["step by step reasoning"]
        assert result.thinking == "step by step reasoning"

    @pytest.mark.asyncio
    async def test_mock_provider_no_callback_still_works(self) -> None:
        """Omitting on_thinking should not break anything."""
        from tests.conftest import MockLLMProvider

        provider = MockLLMProvider(
            responses=[LLMResponse(text="answer", thinking="some thinking")]
        )
        result = await provider.complete(
            messages=[{"role": "user", "content": "hi"}],
        )
        assert result.text == "answer"
        assert result.thinking == "some thinking"


# ---------------------------------------------------------------------------
# 3. Claude provider: streaming + thinking (unit tests with mocked SDK)
# ---------------------------------------------------------------------------


class TestClaudeProviderStreaming:
    @pytest.mark.asyncio
    async def test_claude_uses_stream_api(self) -> None:
        """ClaudeProvider.complete() should use client.messages.stream(), not .create()."""
        from agentic_crawler.llm.claude import ClaudeProvider

        provider = ClaudeProvider(api_key="test-key")

        # Build a mock stream context manager
        mock_thinking_block = MagicMock()
        mock_thinking_block.type = "thinking"
        mock_thinking_block.thinking = "my reasoning"

        mock_text_block = MagicMock()
        mock_text_block.type = "text"
        mock_text_block.text = "my answer"

        mock_final_message = MagicMock()
        mock_final_message.content = [mock_thinking_block, mock_text_block]
        mock_final_message.usage.input_tokens = 10
        mock_final_message.usage.output_tokens = 20

        # Mock the stream events for thinking deltas
        mock_event_thinking_start = MagicMock()
        mock_event_thinking_start.type = "content_block_start"
        mock_event_thinking_start.content_block.type = "thinking"

        mock_event_thinking_delta = MagicMock()
        mock_event_thinking_delta.type = "content_block_delta"
        mock_event_thinking_delta.delta.type = "thinking_delta"
        mock_event_thinking_delta.delta.thinking = "my reasoning"

        mock_event_text_delta = MagicMock()
        mock_event_text_delta.type = "content_block_delta"
        mock_event_text_delta.delta.type = "text_delta"
        mock_event_text_delta.delta.text = "my answer"

        mock_stream = AsyncMock()
        mock_stream.__aiter__ = lambda self: _async_iter([
            mock_event_thinking_start,
            mock_event_thinking_delta,
            mock_event_text_delta,
        ])
        mock_stream.get_final_message = AsyncMock(return_value=mock_final_message)

        mock_stream_ctx = AsyncMock()
        mock_stream_ctx.__aenter__ = AsyncMock(return_value=mock_stream)
        mock_stream_ctx.__aexit__ = AsyncMock(return_value=False)

        provider.client.messages.stream = MagicMock(return_value=mock_stream_ctx)

        thinking_chunks: list[str] = []
        result = await provider.complete(
            messages=[{"role": "user", "content": "think about 2+2"}],
            on_thinking=lambda chunk: thinking_chunks.append(chunk),
        )

        # Verify stream was used (not create)
        provider.client.messages.stream.assert_called_once()
        call_kwargs = provider.client.messages.stream.call_args[1]

        # Verify thinking is enabled
        assert "thinking" in call_kwargs
        assert call_kwargs["thinking"]["type"] == "enabled"
        assert call_kwargs["thinking"]["budget_tokens"] > 0

        # Verify thinking callback was called
        assert thinking_chunks == ["my reasoning"]

        # Verify response has thinking
        assert result.thinking == "my reasoning"
        assert result.text == "my answer"

        await provider.close()

    @pytest.mark.asyncio
    async def test_claude_stream_does_not_set_temperature(self) -> None:
        """Extended thinking requires temperature not be set (API constraint)."""
        from agentic_crawler.llm.claude import ClaudeProvider

        provider = ClaudeProvider(api_key="test-key")

        mock_final_message = MagicMock()
        mock_final_message.content = []
        mock_final_message.usage.input_tokens = 0
        mock_final_message.usage.output_tokens = 0

        mock_stream = AsyncMock()
        mock_stream.__aiter__ = lambda self: _async_iter([])
        mock_stream.get_final_message = AsyncMock(return_value=mock_final_message)

        mock_stream_ctx = AsyncMock()
        mock_stream_ctx.__aenter__ = AsyncMock(return_value=mock_stream)
        mock_stream_ctx.__aexit__ = AsyncMock(return_value=False)

        provider.client.messages.stream = MagicMock(return_value=mock_stream_ctx)

        await provider.complete(
            messages=[{"role": "user", "content": "test"}],
            temperature=0.5,
        )

        call_kwargs = provider.client.messages.stream.call_args[1]
        # Temperature should NOT be in kwargs when thinking is enabled
        assert "temperature" not in call_kwargs

        await provider.close()


# ---------------------------------------------------------------------------
# 3b. OpenAI provider: streaming + reasoning support
# ---------------------------------------------------------------------------


class TestOpenAIProviderStreaming:
    @pytest.mark.asyncio
    async def test_openai_chat_api_uses_streaming(self) -> None:
        """OpenAI Chat API should use streaming (stream=True)."""
        from agentic_crawler.llm.openai import OpenAIProvider

        provider = OpenAIProvider(api_key="test-key")

        # Mock the streaming response
        mock_chunk_reasoning = MagicMock()
        mock_chunk_reasoning.choices = [MagicMock()]
        mock_chunk_reasoning.choices[0].delta.content = None
        mock_chunk_reasoning.choices[0].delta.tool_calls = None
        mock_chunk_reasoning.choices[0].delta.reasoning_content = "let me think"

        mock_chunk_text = MagicMock()
        mock_chunk_text.choices = [MagicMock()]
        mock_chunk_text.choices[0].delta.content = "the answer"
        mock_chunk_text.choices[0].delta.tool_calls = None
        mock_chunk_text.choices[0].delta.reasoning_content = None

        mock_chunk_done = MagicMock()
        mock_chunk_done.choices = [MagicMock()]
        mock_chunk_done.choices[0].delta.content = None
        mock_chunk_done.choices[0].delta.tool_calls = None
        mock_chunk_done.choices[0].delta.reasoning_content = None
        mock_chunk_done.usage = MagicMock()
        mock_chunk_done.usage.prompt_tokens = 10
        mock_chunk_done.usage.completion_tokens = 20

        async def mock_stream():
            for chunk in [mock_chunk_reasoning, mock_chunk_text, mock_chunk_done]:
                yield chunk

        provider.client.chat.completions.create = AsyncMock(return_value=mock_stream())

        thinking_chunks: list[str] = []
        result = await provider.complete(
            messages=[{"role": "user", "content": "test"}],
            on_thinking=lambda c: thinking_chunks.append(c),
        )

        # Verify stream=True was passed
        provider.client.chat.completions.create.assert_called_once()
        call_kwargs = provider.client.chat.completions.create.call_args[1]
        assert call_kwargs.get("stream") is True
        assert call_kwargs.get("stream_options") == {"include_usage": True}

        # Verify reasoning was captured
        assert thinking_chunks == ["let me think"]
        assert result.thinking == "let me think"
        assert result.text == "the answer"

        await provider.close()

    @pytest.mark.asyncio
    async def test_codex_api_captures_reasoning_events(self) -> None:
        """Codex SSE stream should capture reasoning_summary_text.delta events."""
        from agentic_crawler.llm.openai import OpenAIProvider

        provider = OpenAIProvider(api_key="unused", model="o4-mini", use_oauth=False)
        provider._use_oauth = True  # Force codex path without actual tokens
        provider._access_token = "fake.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoiYWNjXzEyMyJ9fQ.sig"

        # Build SSE lines simulating reasoning + text + tool call
        sse_lines = [
            'data: {"type":"response.reasoning_summary_text.delta","delta":"thinking step 1"}',
            'data: {"type":"response.reasoning_summary_text.delta","delta":" and step 2"}',
            'data: {"type":"response.reasoning_summary_text.done","text":"thinking step 1 and step 2"}',
            'data: {"type":"response.output_text.delta","delta":"my answer"}',
            'data: {"type":"response.output_text.done","text":"my answer"}',
            'data: {"type":"response.completed","response":{"usage":{"input_tokens":5,"output_tokens":10}}}',
            "data: [DONE]",
        ]

        mock_resp = AsyncMock()
        mock_resp.status_code = 200
        mock_resp.aiter_lines = lambda: _async_iter(sse_lines)

        mock_stream_ctx = AsyncMock()
        mock_stream_ctx.__aenter__ = AsyncMock(return_value=mock_resp)
        mock_stream_ctx.__aexit__ = AsyncMock(return_value=False)

        mock_client = AsyncMock()
        mock_client.stream = MagicMock(return_value=mock_stream_ctx)
        provider._get_http_client = AsyncMock(return_value=mock_client)

        thinking_chunks: list[str] = []
        result = await provider._complete_codex_api(
            messages=[{"role": "user", "content": "test"}],
            tools=None,
            temperature=0.0,
            on_thinking=lambda c: thinking_chunks.append(c),
        )

        assert thinking_chunks == ["thinking step 1", " and step 2"]
        assert result.thinking == "thinking step 1 and step 2"
        assert result.text == "my answer"


# ---------------------------------------------------------------------------
# 4. Agent loop passes thinking callback to provider
# ---------------------------------------------------------------------------


class TestAgentLoopThinkingDisplay:
    @pytest.mark.asyncio
    async def test_loop_passes_on_thinking_to_provider(self) -> None:
        """run_agent should pass an on_thinking callback to provider.complete()."""
        from tests.conftest import MockLLMProvider

        provider = MockLLMProvider(
            responses=[
                LLMResponse(text="1. Navigate to site\n2. Extract data\n3. Done"),
                LLMResponse(
                    thinking="I should call done",
                    tool_calls=[ToolCall(id="d1", name="done", arguments={"summary": "finished"})],
                ),
            ]
        )

        # We verify that complete() was called with on_thinking kwarg
        original_complete = provider.complete
        on_thinking_received = []

        async def tracking_complete(*args: Any, **kwargs: Any) -> LLMResponse:
            if "on_thinking" in kwargs:
                on_thinking_received.append(kwargs["on_thinking"])
            return await original_complete(*args, **kwargs)

        provider.complete = tracking_complete  # type: ignore[assignment]

        from agentic_crawler.agent.loop import run_agent
        from agentic_crawler.config import Settings

        settings = Settings(llm_provider="claude", anthropic_api_key="fake")

        with patch("agentic_crawler.agent.loop.get_provider", return_value=provider), \
             patch("agentic_crawler.agent.loop.FetcherRouter") as mock_router_cls:
            mock_router = AsyncMock()
            mock_router_cls.return_value = mock_router
            mock_router.close = AsyncMock()

            await run_agent("test goal", settings, verbose=True)

        # Both planning and execution phases should have passed on_thinking
        # Call 1 = planning, Call 2 = execution
        assert len(on_thinking_received) >= 2
        assert all(callable(cb) for cb in on_thinking_received)


# ---------------------------------------------------------------------------
# 5. Integration test — real Claude API
# ---------------------------------------------------------------------------


@pytest.mark.integration
class TestClaudeStreamingIntegration:
    @pytest.mark.asyncio
    async def test_claude_streaming_with_thinking_real_api(self) -> None:
        """Hit the real Claude API with streaming + extended thinking."""
        from agentic_crawler.config import Settings

        settings = Settings()
        api_key = settings.anthropic_api_key
        if not api_key:
            pytest.skip("ANTHROPIC_API_KEY not set")

        from agentic_crawler.llm.claude import ClaudeProvider

        provider = ClaudeProvider(api_key=api_key)

        thinking_chunks: list[str] = []
        result = await provider.complete(
            messages=[
                {"role": "user", "content": "What is 15 * 37? Show your reasoning."}
            ],
            on_thinking=lambda chunk: thinking_chunks.append(chunk),
        )

        try:
            # Response should have both thinking and text
            assert result.text is not None, "Expected text in response"
            assert result.thinking is not None, "Expected thinking in response"
            assert len(result.thinking) > 0, "Thinking should not be empty"

            # Thinking should have been streamed via callback
            assert len(thinking_chunks) > 0, "Expected thinking chunks from streaming"

            # Concatenated chunks should equal full thinking
            assert "".join(thinking_chunks) == result.thinking

            # Usage should be tracked
            assert result.usage.get("input_tokens", 0) > 0
            assert result.usage.get("output_tokens", 0) > 0

            # The answer should contain 555
            assert "555" in result.text
        finally:
            await provider.close()

    @pytest.mark.asyncio
    async def test_claude_streaming_with_tools_real_api(self) -> None:
        """Verify streaming works when tools are provided."""
        from agentic_crawler.config import Settings

        settings = Settings()
        api_key = settings.anthropic_api_key
        if not api_key:
            pytest.skip("ANTHROPIC_API_KEY not set")

        from agentic_crawler.llm.claude import ClaudeProvider

        provider = ClaudeProvider(api_key=api_key)

        tools = [
            {
                "name": "get_weather",
                "description": "Get the weather for a location",
                "parameters": {
                    "type": "object",
                    "properties": {"location": {"type": "string"}},
                    "required": ["location"],
                },
            }
        ]

        thinking_chunks: list[str] = []
        result = await provider.complete(
            messages=[
                {"role": "user", "content": "What's the weather in Tokyo?"}
            ],
            tools=tools,
            on_thinking=lambda chunk: thinking_chunks.append(chunk),
        )

        try:
            # Should produce a tool call
            assert result.has_tool_calls, "Expected tool call for weather query"
            assert result.tool_calls[0].name == "get_weather"
            assert "tokyo" in result.tool_calls[0].arguments.get("location", "").lower()

            # Thinking should still be captured
            assert result.thinking is not None
            assert len(thinking_chunks) > 0
        finally:
            await provider.close()
