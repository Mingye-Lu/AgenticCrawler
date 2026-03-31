from __future__ import annotations

import uuid
from typing import Any, cast
from unittest.mock import AsyncMock, patch

import pytest

from agentic_crawler.agent.manager import AgentManager
from agentic_crawler.agent.state import AgentState
from agentic_crawler.config import Settings
from agentic_crawler.fetcher.base import FetchResult
from agentic_crawler.llm.base import LLMResponse, ToolCall
from tests.conftest import MockLLMProvider


class MockFetcherRouter:
    def __init__(self) -> None:
        self.closed = False
        self.get_calls: list[str] = []
        self.http = None
        self.browser = None
        self._using_browser = False

    @property
    def needs_browser(self) -> bool:
        return self._using_browser

    def escalate_to_browser(self) -> None:
        self._using_browser = True

    def should_use_browser(self, action_name: str) -> bool:
        return action_name in {"click", "fill_form", "scroll", "screenshot", "wait"}

    async def ensure_browser_ready(self) -> None:
        return None

    async def get(self, url: str) -> FetchResult:
        self.get_calls.append(url)
        return FetchResult(url=url, status_code=200, html="<html><body>ok</body></html>")

    async def close(self) -> None:
        self.closed = True


def _manager() -> AgentManager:
    return AgentManager(max_concurrent_per_parent=5, max_depth=3, max_total=10)


def _router() -> Any:
    return cast(Any, MockFetcherRouter())


@pytest.mark.asyncio
async def test_crawl_agent_runs_to_completion() -> None:
    from agentic_crawler.agent.crawl_agent import CrawlAgent

    provider = MockLLMProvider(
        responses=[
            LLMResponse(text="1. Navigate\n2. Extract\n3. Done"),
            LLMResponse(
                tool_calls=[
                    ToolCall(id="nav1", name="navigate", arguments={"url": "https://example.com"})
                ]
            ),
            LLMResponse(
                tool_calls=[ToolCall(id="done1", name="done", arguments={"summary": "finished"})]
            ),
        ]
    )
    state = AgentState(goal="crawl", max_steps=10)
    router = _router()
    agent = CrawlAgent(
        agent_id="root-1",
        state=state,
        settings=Settings(),
        provider=provider,
        manager=_manager(),
        router=router,
        is_root=True,
    )

    await agent.run()

    assert state.done is True
    assert state.done_reason == "finished"


@pytest.mark.asyncio
async def test_crawl_agent_extracts_data() -> None:
    from agentic_crawler.agent.crawl_agent import CrawlAgent

    payload = {"name": "Widget", "price": "$10"}
    provider = MockLLMProvider(
        responses=[
            LLMResponse(text="1. Extract data\n2. Done"),
            LLMResponse(
                tool_calls=[
                    ToolCall(
                        id="extract1",
                        name="extract_data",
                        arguments={"instruction": "Extract product", "data": payload},
                    ),
                    ToolCall(id="done1", name="done", arguments={"summary": "finished"}),
                ]
            ),
        ]
    )
    state = AgentState(goal="extract", max_steps=10)
    agent = CrawlAgent(
        agent_id="root-2",
        state=state,
        settings=Settings(),
        provider=provider,
        manager=_manager(),
        router=_router(),
        is_root=True,
    )

    await agent.run()

    assert state.extracted_data == [payload]


@pytest.mark.asyncio
async def test_crawl_agent_respects_max_steps() -> None:
    from agentic_crawler.agent.crawl_agent import CrawlAgent

    provider = MockLLMProvider(
        responses=[
            LLMResponse(text="1. Navigate forever"),
            LLMResponse(
                tool_calls=[
                    ToolCall(id="nav1", name="navigate", arguments={"url": "https://example.com/1"})
                ]
            ),
            LLMResponse(
                tool_calls=[
                    ToolCall(id="nav2", name="navigate", arguments={"url": "https://example.com/2"})
                ]
            ),
            LLMResponse(
                tool_calls=[
                    ToolCall(id="nav3", name="navigate", arguments={"url": "https://example.com/3"})
                ]
            ),
            LLMResponse(
                tool_calls=[
                    ToolCall(id="nav4", name="navigate", arguments={"url": "https://example.com/4"})
                ]
            ),
        ]
    )
    state = AgentState(goal="never done", max_steps=3)
    agent = CrawlAgent(
        agent_id="root-3",
        state=state,
        settings=Settings(max_steps=3),
        provider=provider,
        manager=_manager(),
        router=_router(),
        is_root=True,
    )

    await agent.run()

    assert state.step_count == 3
    assert state.done is False


def test_crawl_agent_has_unique_id() -> None:
    from agentic_crawler.agent.crawl_agent import CrawlAgent

    provider = MockLLMProvider()
    manager = _manager()
    router = _router()
    agent_1 = CrawlAgent(
        agent_id=str(uuid.uuid4()),
        state=AgentState(goal="goal one"),
        settings=Settings(),
        provider=provider,
        manager=manager,
        router=router,
        is_root=True,
    )
    agent_2 = CrawlAgent(
        agent_id=str(uuid.uuid4()),
        state=AgentState(goal="goal two"),
        settings=Settings(),
        provider=provider,
        manager=manager,
        router=router,
        is_root=False,
    )

    assert agent_1.agent_id != agent_2.agent_id


@pytest.mark.asyncio
async def test_crawl_agent_root_uses_fakebroswer() -> None:
    from agentic_crawler.agent.crawl_agent import CrawlAgent

    provider = MockLLMProvider(
        responses=[
            LLMResponse(text="1. Done"),
            LLMResponse(
                tool_calls=[ToolCall(id="done1", name="done", arguments={"summary": "done"})]
            ),
        ]
    )
    state = AgentState(goal="root", max_steps=5)
    manager = _manager()

    with patch("agentic_crawler.agent.crawl_agent.FetcherRouter") as router_cls:
        router = AsyncMock()
        router.close = AsyncMock()
        router_cls.return_value = router

        agent = CrawlAgent(
            agent_id="root-4",
            state=state,
            settings=Settings(headless=True, browser_timeout=12345),
            provider=provider,
            manager=manager,
            router=None,
            is_root=True,
        )
        await agent.run()

    router_cls.assert_called_once_with(headless=True, browser_timeout=12345)


@pytest.mark.asyncio
async def test_crawl_agent_child_skips_planning() -> None:
    from agentic_crawler.agent.crawl_agent import CrawlAgent

    provider = MockLLMProvider(
        responses=[
            LLMResponse(
                tool_calls=[ToolCall(id="done1", name="done", arguments={"summary": "done"})]
            )
        ]
    )
    state = AgentState(goal="child", max_steps=5)

    with patch("agentic_crawler.agent.crawl_agent._plan", new_callable=AsyncMock) as mock_plan:
        mock_plan.return_value = ["should not be used"]
        agent = CrawlAgent(
            agent_id="child-1",
            state=state,
            settings=Settings(),
            provider=provider,
            manager=_manager(),
            router=_router(),
            is_root=False,
        )
        await agent.run()

    mock_plan.assert_not_awaited()
