from __future__ import annotations

from collections.abc import Callable
from typing import Any

import pytest

from agentic_crawler.llm.base import LLMProvider, LLMResponse, ToolCall


class MockLLMProvider:
    """LLM provider that returns scripted responses for testing."""

    def __init__(self, responses: list[LLMResponse] | None = None) -> None:
        self.responses = list(responses or [])
        self.call_count = 0
        self.messages_log: list[list[dict[str, Any]]] = []

    def add_response(self, response: LLMResponse) -> None:
        self.responses.append(response)

    async def complete(
        self,
        messages: list[dict[str, Any]],
        tools: list[dict[str, Any]] | None = None,
        temperature: float = 0.0,
        on_thinking: Callable[[str], None] | None = None,
    ) -> LLMResponse:
        self.messages_log.append(messages)
        if self.call_count < len(self.responses):
            response = self.responses[self.call_count]
            self.call_count += 1
            # Fire the thinking callback if the response has thinking
            if on_thinking and response.thinking:
                on_thinking(response.thinking)
            return response
        # Default: signal done
        return LLMResponse(
            tool_calls=[ToolCall(id="done_1", name="done", arguments={"summary": "No more scripted responses"})],
        )

    async def close(self) -> None:
        pass


@pytest.fixture
def mock_llm() -> MockLLMProvider:
    return MockLLMProvider()


SAMPLE_HTML = """\
<!DOCTYPE html>
<html>
<head><title>Test Page</title></head>
<body>
  <h1>Welcome to Test Page</h1>
  <p>This is a test page with some content.</p>
  <a href="/about">About</a>
  <a href="/products">Products</a>
  <form action="/search" method="get">
    <input type="text" name="q" placeholder="Search...">
    <button type="submit">Search</button>
  </form>
  <table>
    <tr><th>Name</th><th>Price</th></tr>
    <tr><td>Widget A</td><td>$10.00</td></tr>
    <tr><td>Widget B</td><td>$20.00</td></tr>
  </table>
</body>
</html>
"""


@pytest.fixture
def sample_html() -> str:
    return SAMPLE_HTML
