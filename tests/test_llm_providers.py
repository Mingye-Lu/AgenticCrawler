import pytest

from agentic_crawler.llm.base import LLMResponse, ToolCall
from tests.conftest import MockLLMProvider


@pytest.mark.asyncio
async def test_mock_llm_returns_scripted_responses() -> None:
    provider = MockLLMProvider(responses=[
        LLMResponse(text="Hello world"),
        LLMResponse(tool_calls=[ToolCall(id="1", name="navigate", arguments={"url": "https://example.com"})]),
    ])

    r1 = await provider.complete(messages=[{"role": "user", "content": "hi"}])
    assert r1.text == "Hello world"
    assert not r1.has_tool_calls

    r2 = await provider.complete(messages=[{"role": "user", "content": "go"}])
    assert r2.has_tool_calls
    assert r2.tool_calls[0].name == "navigate"

    # After exhausting scripted responses, returns done
    r3 = await provider.complete(messages=[{"role": "user", "content": "more"}])
    assert r3.has_tool_calls
    assert r3.tool_calls[0].name == "done"


@pytest.mark.asyncio
async def test_mock_llm_logs_messages() -> None:
    provider = MockLLMProvider(responses=[LLMResponse(text="ok")])
    msgs = [{"role": "user", "content": "test"}]
    await provider.complete(messages=msgs)
    assert len(provider.messages_log) == 1
    assert provider.messages_log[0] == msgs
