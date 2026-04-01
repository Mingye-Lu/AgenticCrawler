from __future__ import annotations

from typing import Any
from unittest.mock import AsyncMock

import pytest

from agentic_crawler.llm.claude import ClaudeProvider
from agentic_crawler.llm.openai import OpenAIProvider


class _FakeClaudeTextEvent:
    def __init__(self, text: str) -> None:
        self.type = "text"
        self.text = text


class _FakeClaudeThinkingDelta:
    def __init__(self, thinking: str) -> None:
        self.type = "thinking_delta"
        self.thinking = thinking


class _FakeClaudeBlockDeltaEvent:
    def __init__(self, delta: Any) -> None:
        self.type = "content_block_delta"
        self.delta = delta


class _FakeContentBlock:
    def __init__(self, block_type: str, **kwargs: Any) -> None:
        self.type = block_type
        for k, v in kwargs.items():
            setattr(self, k, v)


class _FakeUsage:
    def __init__(self, input_tokens: int = 10, output_tokens: int = 20) -> None:
        self.input_tokens = input_tokens
        self.output_tokens = output_tokens


class _FakeFinalMessage:
    def __init__(
        self,
        content: list[Any],
        usage: _FakeUsage | None = None,
    ) -> None:
        self.content = content
        self.usage = usage or _FakeUsage()


class _FakeClaudeStream:
    def __init__(self, events: list[Any], final_message: _FakeFinalMessage) -> None:
        self._events = events
        self._final = final_message
        self._idx = 0

    async def __aenter__(self) -> _FakeClaudeStream:
        return self

    async def __aexit__(self, *args: object) -> None:
        pass

    def __aiter__(self) -> _FakeClaudeStream:
        self._idx = 0
        return self

    async def __anext__(self) -> Any:
        if self._idx >= len(self._events):
            raise StopAsyncIteration
        event = self._events[self._idx]
        self._idx += 1
        return event

    async def get_final_message(self) -> _FakeFinalMessage:
        return self._final


class _FakeChoiceDelta:
    def __init__(
        self,
        content: str | None = None,
        tool_calls: Any = None,
        reasoning_content: str | None = None,
    ) -> None:
        self.content = content
        self.tool_calls = tool_calls
        self.reasoning_content = reasoning_content


class _FakeChoice:
    def __init__(self, delta: _FakeChoiceDelta) -> None:
        self.delta = delta


class _FakeChunkUsage:
    def __init__(self, prompt_tokens: int = 10, completion_tokens: int = 20) -> None:
        self.prompt_tokens = prompt_tokens
        self.completion_tokens = completion_tokens


class _FakeChunk:
    def __init__(
        self,
        choices: list[_FakeChoice] | None = None,
        usage: _FakeChunkUsage | None = None,
    ) -> None:
        self.choices = choices or []
        self.usage = usage


async def _fake_openai_stream(chunks: list[_FakeChunk]) -> Any:
    for chunk in chunks:
        yield chunk


def _make_claude_provider() -> ClaudeProvider:
    return ClaudeProvider(model="test-model", api_key="fake-key")


def _patch_claude_stream(
    provider: ClaudeProvider,
    events: list[Any],
    final: _FakeFinalMessage,
) -> Any:
    from unittest.mock import patch

    return patch.object(
        provider.client.messages,
        "stream",
        new=lambda **kw: _FakeClaudeStream(events, final),
    )


@pytest.mark.asyncio
async def test_claude_on_text_delta_called_with_chunks() -> None:
    events = [
        _FakeClaudeTextEvent("Hello"),
        _FakeClaudeTextEvent(", world"),
        _FakeClaudeTextEvent("!"),
    ]
    final = _FakeFinalMessage(
        content=[_FakeContentBlock("text", text="Hello, world!")],
    )

    provider = _make_claude_provider()
    collected: list[str] = []

    with _patch_claude_stream(provider, events, final):
        result = await provider.complete(
            messages=[{"role": "user", "content": "hi"}],
            on_text_delta=lambda chunk: collected.append(chunk),
        )

    assert collected == ["Hello", ", world", "!"]
    assert result.text == "Hello, world!"


@pytest.mark.asyncio
async def test_claude_on_text_delta_none_backward_compat() -> None:
    events = [_FakeClaudeTextEvent("Hello")]
    final = _FakeFinalMessage(
        content=[_FakeContentBlock("text", text="Hello")],
    )

    provider = _make_claude_provider()

    with _patch_claude_stream(provider, events, final):
        result = await provider.complete(
            messages=[{"role": "user", "content": "hi"}],
        )

    assert result.text == "Hello"


@pytest.mark.asyncio
async def test_claude_on_text_delta_and_on_thinking_coexist() -> None:
    events = [
        _FakeClaudeBlockDeltaEvent(
            _FakeClaudeThinkingDelta("Thinking..."),
        ),
        _FakeClaudeTextEvent("Answer"),
    ]
    final = _FakeFinalMessage(
        content=[_FakeContentBlock("text", text="Answer")],
    )

    provider = _make_claude_provider()
    thinking_chunks: list[str] = []
    text_chunks: list[str] = []

    with _patch_claude_stream(provider, events, final):
        result = await provider.complete(
            messages=[{"role": "user", "content": "think and answer"}],
            on_thinking=lambda c: thinking_chunks.append(c),
            on_text_delta=lambda c: text_chunks.append(c),
        )

    assert thinking_chunks == ["Thinking..."]
    assert text_chunks == ["Answer"]
    assert result.thinking == "Thinking..."
    assert result.text == "Answer"


@pytest.mark.asyncio
async def test_claude_response_text_accumulated_after_streaming() -> None:
    events = [
        _FakeClaudeTextEvent("chunk1"),
        _FakeClaudeTextEvent("chunk2"),
        _FakeClaudeTextEvent("chunk3"),
    ]
    final = _FakeFinalMessage(
        content=[_FakeContentBlock("text", text="chunk1chunk2chunk3")],
    )

    provider = _make_claude_provider()
    deltas: list[str] = []

    with _patch_claude_stream(provider, events, final):
        result = await provider.complete(
            messages=[{"role": "user", "content": "hi"}],
            on_text_delta=lambda c: deltas.append(c),
        )

    assert deltas == ["chunk1", "chunk2", "chunk3"]
    assert result.text == "chunk1chunk2chunk3"


def _make_openai_provider() -> OpenAIProvider:
    return OpenAIProvider(api_key="fake-key", model="test-model")


@pytest.mark.asyncio
async def test_openai_on_text_delta_called_with_chunks() -> None:
    chunks = [
        _FakeChunk(choices=[_FakeChoice(_FakeChoiceDelta(content="Hello"))]),
        _FakeChunk(choices=[_FakeChoice(_FakeChoiceDelta(content=" world"))]),
        _FakeChunk(choices=[], usage=_FakeChunkUsage(10, 20)),
    ]

    provider = _make_openai_provider()
    collected: list[str] = []

    mock_create = AsyncMock(return_value=_fake_openai_stream(chunks))
    with pytest.MonkeyPatch.context() as mp:
        mp.setattr(provider.client.chat.completions, "create", mock_create)
        result = await provider.complete(
            messages=[{"role": "user", "content": "hi"}],
            on_text_delta=lambda chunk: collected.append(chunk),
        )

    assert collected == ["Hello", " world"]
    assert result.text == "Hello world"


@pytest.mark.asyncio
async def test_openai_on_text_delta_none_backward_compat() -> None:
    chunks = [
        _FakeChunk(choices=[_FakeChoice(_FakeChoiceDelta(content="Hello"))]),
        _FakeChunk(choices=[], usage=_FakeChunkUsage(10, 20)),
    ]

    provider = _make_openai_provider()

    mock_create = AsyncMock(return_value=_fake_openai_stream(chunks))
    with pytest.MonkeyPatch.context() as mp:
        mp.setattr(provider.client.chat.completions, "create", mock_create)
        result = await provider.complete(
            messages=[{"role": "user", "content": "hi"}],
        )

    assert result.text == "Hello"


@pytest.mark.asyncio
async def test_openai_on_text_delta_and_on_thinking_coexist() -> None:
    chunks = [
        _FakeChunk(
            choices=[
                _FakeChoice(
                    _FakeChoiceDelta(reasoning_content="Reasoning..."),
                )
            ]
        ),
        _FakeChunk(choices=[_FakeChoice(_FakeChoiceDelta(content="Answer"))]),
        _FakeChunk(choices=[], usage=_FakeChunkUsage(10, 20)),
    ]

    provider = _make_openai_provider()
    thinking_chunks: list[str] = []
    text_chunks: list[str] = []

    mock_create = AsyncMock(return_value=_fake_openai_stream(chunks))
    with pytest.MonkeyPatch.context() as mp:
        mp.setattr(provider.client.chat.completions, "create", mock_create)
        result = await provider.complete(
            messages=[{"role": "user", "content": "think and answer"}],
            on_thinking=lambda c: thinking_chunks.append(c),
            on_text_delta=lambda c: text_chunks.append(c),
        )

    assert thinking_chunks == ["Reasoning..."]
    assert text_chunks == ["Answer"]
    assert result.thinking == "Reasoning..."
    assert result.text == "Answer"


@pytest.mark.asyncio
async def test_openai_response_text_accumulated_after_streaming() -> None:
    chunks = [
        _FakeChunk(choices=[_FakeChoice(_FakeChoiceDelta(content="a"))]),
        _FakeChunk(choices=[_FakeChoice(_FakeChoiceDelta(content="b"))]),
        _FakeChunk(choices=[_FakeChoice(_FakeChoiceDelta(content="c"))]),
        _FakeChunk(choices=[], usage=_FakeChunkUsage(10, 20)),
    ]

    provider = _make_openai_provider()
    deltas: list[str] = []

    mock_create = AsyncMock(return_value=_fake_openai_stream(chunks))
    with pytest.MonkeyPatch.context() as mp:
        mp.setattr(provider.client.chat.completions, "create", mock_create)
        result = await provider.complete(
            messages=[{"role": "user", "content": "hi"}],
            on_text_delta=lambda c: deltas.append(c),
        )

    assert deltas == ["a", "b", "c"]
    assert result.text == "abc"
