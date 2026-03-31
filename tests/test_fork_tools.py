from __future__ import annotations

import asyncio
import io
from typing import Any, cast
from unittest.mock import patch

import pytest
from rich.console import Console

from agentic_crawler.agent.crawl_agent import CrawlAgent
from agentic_crawler.agent.display import ConsoleDisplay
from agentic_crawler.agent.manager import AgentManager
from agentic_crawler.agent.state import AgentState
from agentic_crawler.agent.tools import get_tool_schemas
from agentic_crawler.config import Settings
from agentic_crawler.fetcher.base import FetchResult
from agentic_crawler.llm.base import LLMResponse, ToolCall
from tests.conftest import MockLLMProvider


class MockFetcherRouter:
    def __init__(self) -> None:
        self._using_browser = False
        self._last_url: str | None = None
        self.closed = False

    @property
    def needs_browser(self) -> bool:
        return self._using_browser

    def escalate_to_browser(self) -> None:
        self._using_browser = True

    def should_use_browser(self, action_name: str) -> bool:
        return False

    async def ensure_browser_ready(self) -> None:
        pass

    async def get(self, url: str) -> FetchResult:
        self._last_url = url
        return FetchResult(url=url, status_code=200, html="<html><body>test</body></html>")

    async def close(self) -> None:
        self.closed = True


def _make_manager(**kwargs: Any) -> AgentManager:
    defaults: dict[str, int] = {
        "max_concurrent_per_parent": 5,
        "max_depth": 3,
        "max_total": 10,
    }
    defaults.update(kwargs)
    return AgentManager(**defaults)


def _plan_response(text: str = "1. Do it\n2. Done") -> LLMResponse:
    return LLMResponse(text=text)


def _fork_call(sub_goal: str = "search subpage", url: str | None = None) -> LLMResponse:
    args: dict[str, Any] = {"sub_goal": sub_goal}
    if url is not None:
        args["url"] = url
    return LLMResponse(tool_calls=[ToolCall(id="fork1", name="fork", arguments=args)])


def _done_call(summary: str = "done") -> LLMResponse:
    return LLMResponse(
        tool_calls=[ToolCall(id="done1", name="done", arguments={"summary": summary})]
    )


def _display(agent_id: str, is_root: bool) -> ConsoleDisplay:
    return ConsoleDisplay(
        console=Console(file=io.StringIO(), force_terminal=False),
        agent_id=agent_id,
        is_root=is_root,
    )


async def _await_children(manager: AgentManager, parent_id: str) -> None:
    tasks = manager.get_child_tasks(parent_id)
    if tasks:
        await asyncio.gather(*tasks, return_exceptions=True)


# ── Schema tests ─────────────────────────────────────────────────────


def test_fork_tool_schema_registered() -> None:
    schemas = get_tool_schemas()
    names = {s["name"] for s in schemas}
    assert "fork" in names


def test_fork_tool_schema_has_correct_params() -> None:
    schemas = get_tool_schemas()
    fork_schema = next(s for s in schemas if s["name"] == "fork")
    params = fork_schema["parameters"]

    assert "sub_goal" in params["properties"]
    assert "url" in params["properties"]
    assert params["required"] == ["sub_goal"]
    assert "url" not in params["required"]


@pytest.mark.asyncio
async def test_fork_dispatched_as_special_case() -> None:
    """MockLLM returns fork tool call -> child agent spawned, manager tracks 1 child."""
    provider = MockLLMProvider(
        responses=[_plan_response(), _fork_call("search subpage"), _done_call()]
    )
    manager = _make_manager()
    state = AgentState(goal="test fork", max_steps=10)

    with patch("agentic_crawler.agent.crawl_agent.FetcherRouter") as router_cls:
        router_cls.return_value = MockFetcherRouter()
        agent = CrawlAgent(
            agent_id="root-fork-1",
            state=state,
            settings=Settings(),
            provider=provider,
            manager=manager,
            router=cast(Any, MockFetcherRouter()),
            is_root=True,
            display=_display("root-fork-1", True),
        )
        await agent.run()
        await _await_children(manager, "root-fork-1")

    children = manager.get_children("root-fork-1")
    assert len(children) == 1


@pytest.mark.asyncio
async def test_fork_returns_observation_to_parent() -> None:
    """After fork, parent state.history contains a step with 'Forked' in observation."""
    provider = MockLLMProvider(
        responses=[_plan_response(), _fork_call("search subpage"), _done_call()]
    )
    manager = _make_manager()
    state = AgentState(goal="test fork obs", max_steps=10)

    with patch("agentic_crawler.agent.crawl_agent.FetcherRouter") as router_cls:
        router_cls.return_value = MockFetcherRouter()
        agent = CrawlAgent(
            agent_id="root-fork-2",
            state=state,
            settings=Settings(),
            provider=provider,
            manager=manager,
            router=cast(Any, MockFetcherRouter()),
            is_root=True,
            display=_display("root-fork-2", True),
        )
        await agent.run()
        await _await_children(manager, "root-fork-2")

    fork_steps = [s for s in state.history if s.action == "fork"]
    assert len(fork_steps) == 1
    assert "Forked" in fork_steps[0].observation
    assert fork_steps[0].success is True


@pytest.mark.asyncio
async def test_fork_at_limit_returns_error_observation() -> None:
    """Manager with max_concurrent_per_parent=0 -> fork returns error, no exception."""
    provider = MockLLMProvider(responses=[_plan_response(), _fork_call("will fail"), _done_call()])
    manager = _make_manager(max_concurrent_per_parent=0)
    state = AgentState(goal="test fork limit", max_steps=10)

    agent = CrawlAgent(
        agent_id="root-fork-3",
        state=state,
        settings=Settings(),
        provider=provider,
        manager=manager,
        router=cast(Any, MockFetcherRouter()),
        is_root=True,
        display=_display("root-fork-3", True),
    )
    await agent.run()

    fork_steps = [s for s in state.history if s.action == "fork"]
    assert len(fork_steps) == 1
    assert "Cannot fork" in fork_steps[0].observation
    assert fork_steps[0].success is False


@pytest.mark.asyncio
async def test_fork_with_url_navigates_child() -> None:
    """Fork with url='https://example.com' -> child state.current_url matches."""
    provider = MockLLMProvider(
        responses=[
            _plan_response(),
            _fork_call("check example", url="https://example.com"),
            _done_call(),
        ]
    )
    manager = _make_manager()
    state = AgentState(goal="test fork url", max_steps=10)

    with patch("agentic_crawler.agent.crawl_agent.FetcherRouter") as router_cls:
        router_cls.return_value = MockFetcherRouter()
        agent = CrawlAgent(
            agent_id="root-fork-4",
            state=state,
            settings=Settings(),
            provider=provider,
            manager=manager,
            router=cast(Any, MockFetcherRouter()),
            is_root=True,
            display=_display("root-fork-4", True),
        )
        await agent.run()
        await _await_children(manager, "root-fork-4")

    assert hasattr(agent, "_child_agents")
    child_agent = list(agent._child_agents.values())[0]
    assert child_agent.state.current_url == "https://example.com"


@pytest.mark.asyncio
async def test_fork_alongside_other_tool_calls() -> None:
    """MockLLM returns [fork(...), done(...)] -> fork processed non-blocking, done also processed."""
    provider = MockLLMProvider(
        responses=[
            _plan_response(),
            LLMResponse(
                tool_calls=[
                    ToolCall(
                        id="fork1",
                        name="fork",
                        arguments={"sub_goal": "parallel work"},
                    ),
                    ToolCall(
                        id="done1",
                        name="done",
                        arguments={"summary": "all done"},
                    ),
                ]
            ),
        ]
    )
    manager = _make_manager()
    state = AgentState(goal="test fork + done", max_steps=10)

    with patch("agentic_crawler.agent.crawl_agent.FetcherRouter") as router_cls:
        router_cls.return_value = MockFetcherRouter()
        agent = CrawlAgent(
            agent_id="root-fork-5",
            state=state,
            settings=Settings(),
            provider=provider,
            manager=manager,
            router=cast(Any, MockFetcherRouter()),
            is_root=True,
            display=_display("root-fork-5", True),
        )
        await agent.run()
        await _await_children(manager, "root-fork-5")

    # Fork was processed (child spawned)
    children = manager.get_children("root-fork-5")
    assert len(children) == 1

    assert state.done is True
    assert state.done_reason == "all done"


# ── wait_for_subagents schema + dispatch tests (Task 7) ─────────────


def test_wait_tool_schema_registered() -> None:
    schemas = get_tool_schemas()
    names = {s["name"] for s in schemas}
    assert "wait_for_subagents" in names


def test_wait_tool_schema_has_empty_params() -> None:
    schemas = get_tool_schemas()
    wait_schema = next(s for s in schemas if s["name"] == "wait_for_subagents")
    assert wait_schema["parameters"]["properties"] == {}


@pytest.mark.asyncio
async def test_wait_with_no_children_returns_immediately() -> None:
    manager = _make_manager()
    state = AgentState(goal="test wait no children", max_steps=5)
    provider = MockLLMProvider([])

    agent = CrawlAgent(
        agent_id="root-wait-1",
        state=state,
        settings=Settings(fork_wait_timeout=5),
        provider=provider,
        manager=manager,
        router=cast(Any, MockFetcherRouter()),
        is_root=False,
        display=_display("root-wait-1", False),
    )
    manager.register_root("root-wait-1")

    result = await agent._wait_for_children()
    assert result == "No active subagents"


@pytest.mark.asyncio
async def test_wait_dispatched_records_step() -> None:
    provider = MockLLMProvider(
        responses=[
            LLMResponse(tool_calls=[ToolCall(id="w1", name="wait_for_subagents", arguments={})]),
            _done_call(),
        ]
    )
    manager = _make_manager()
    state = AgentState(goal="test wait dispatch", max_steps=5)

    agent = CrawlAgent(
        agent_id="root-wait-2",
        state=state,
        settings=Settings(fork_wait_timeout=5),
        provider=provider,
        manager=manager,
        router=cast(Any, MockFetcherRouter()),
        is_root=False,
        display=_display("root-wait-2", False),
    )
    manager.register_root("root-wait-2")
    await agent.run()

    wait_steps = [s for s in state.history if s.action == "wait_for_subagents"]
    assert len(wait_steps) == 1
    assert "No active subagents" in wait_steps[0].observation
    assert wait_steps[0].success is True


@pytest.mark.asyncio
async def test_done_implicitly_waits_for_children() -> None:
    provider = MockLLMProvider(
        responses=[
            _fork_call("child task"),
            _done_call("parent done"),
        ]
    )
    manager = _make_manager()
    state = AgentState(goal="test done waits", max_steps=10)

    with patch("agentic_crawler.agent.crawl_agent.FetcherRouter") as router_cls:
        router_cls.return_value = MockFetcherRouter()
        agent = CrawlAgent(
            agent_id="root-wait-3",
            state=state,
            settings=Settings(fork_wait_timeout=10),
            provider=provider,
            manager=manager,
            router=cast(Any, MockFetcherRouter()),
            is_root=False,
            display=_display("root-wait-3", False),
        )
        manager.register_root("root-wait-3")
        await agent.run()

    assert state.done is True
    assert state.done_reason == "parent done"
    children = manager.get_children("root-wait-3")
    assert len(children) == 1
    child_info = children[0]
    assert child_info.status == "done"


# ── _merge_child_results tests (Task 8) ─────────────────────────────


@pytest.mark.asyncio
async def test_child_extracted_data_merges_to_parent() -> None:
    manager = _make_manager()
    provider = MockLLMProvider([])

    root_state = AgentState(goal="root")
    root_agent = CrawlAgent(
        agent_id="root-merge-1",
        state=root_state,
        settings=Settings(fork_wait_timeout=5),
        provider=provider,
        manager=manager,
        router=cast(Any, MockFetcherRouter()),
        is_root=False,
        display=_display("root-merge-1", False),
    )
    manager.register_root("root-merge-1")

    child_state = AgentState(goal="child task")
    child_state.extracted_data = [{"item": "data"}]
    child_agent = CrawlAgent(
        agent_id="child-1",
        state=child_state,
        settings=Settings(fork_wait_timeout=5),
        provider=provider,
        manager=manager,
        router=cast(Any, MockFetcherRouter()),
        is_root=False,
        display=_display("child-1", False),
    )
    root_agent._child_agents["child-1"] = child_agent
    manager.register_child("child-1", "root-merge-1", None)

    n = root_agent._merge_child_results("child-1", child_agent)
    assert n == 1
    assert root_state.extracted_data == [{"item": "data"}]
    merge_steps = [s for s in root_state.history if s.action == "__child_merge__"]
    assert len(merge_steps) == 1
    assert "child task" in merge_steps[0].observation


@pytest.mark.asyncio
async def test_multiple_children_data_merges() -> None:
    manager = _make_manager()
    provider = MockLLMProvider([])

    root_state = AgentState(goal="root")
    root_agent = CrawlAgent(
        agent_id="root-merge-2",
        state=root_state,
        settings=Settings(fork_wait_timeout=5),
        provider=provider,
        manager=manager,
        router=cast(Any, MockFetcherRouter()),
        is_root=False,
        display=_display("root-merge-2", False),
    )
    manager.register_root("root-merge-2")

    for i in range(2):
        cid = f"child-{i}"
        cs = AgentState(goal=f"child {i}")
        cs.extracted_data = [{"item": f"data-{i}"}]
        ca = CrawlAgent(
            agent_id=cid,
            state=cs,
            settings=Settings(fork_wait_timeout=5),
            provider=provider,
            manager=manager,
            router=cast(Any, MockFetcherRouter()),
            is_root=False,
            display=_display(cid, False),
        )
        root_agent._child_agents[cid] = ca
        manager.register_child(cid, "root-merge-2", None)
        root_agent._merge_child_results(cid, ca)

    assert len(root_state.extracted_data) == 2
    assert root_state.extracted_data[0] == {"item": "data-0"}
    assert root_state.extracted_data[1] == {"item": "data-1"}


@pytest.mark.asyncio
async def test_merge_preserves_parent_data() -> None:
    manager = _make_manager()
    provider = MockLLMProvider([])

    root_state = AgentState(goal="root")
    root_state.extracted_data = [{"existing": "parent-data"}]
    root_agent = CrawlAgent(
        agent_id="root-merge-3",
        state=root_state,
        settings=Settings(fork_wait_timeout=5),
        provider=provider,
        manager=manager,
        router=cast(Any, MockFetcherRouter()),
        is_root=False,
        display=_display("root-merge-3", False),
    )
    manager.register_root("root-merge-3")

    child_state = AgentState(goal="child task")
    child_state.extracted_data = [{"child": "child-data"}]
    child_agent = CrawlAgent(
        agent_id="child-p",
        state=child_state,
        settings=Settings(fork_wait_timeout=5),
        provider=provider,
        manager=manager,
        router=cast(Any, MockFetcherRouter()),
        is_root=False,
        display=_display("child-p", False),
    )
    root_agent._child_agents["child-p"] = child_agent
    manager.register_child("child-p", "root-merge-3", None)

    root_agent._merge_child_results("child-p", child_agent)
    assert len(root_state.extracted_data) == 2
    assert root_state.extracted_data[0] == {"existing": "parent-data"}
    assert root_state.extracted_data[1] == {"child": "child-data"}


@pytest.mark.asyncio
async def test_merge_with_no_child_data_returns_zero() -> None:
    manager = _make_manager()
    provider = MockLLMProvider([])

    root_state = AgentState(goal="root")
    root_agent = CrawlAgent(
        agent_id="root-merge-4",
        state=root_state,
        settings=Settings(fork_wait_timeout=5),
        provider=provider,
        manager=manager,
        router=cast(Any, MockFetcherRouter()),
        is_root=False,
        display=_display("root-merge-4", False),
    )
    manager.register_root("root-merge-4")

    child_state = AgentState(goal="empty child")
    child_agent = CrawlAgent(
        agent_id="child-empty",
        state=child_state,
        settings=Settings(fork_wait_timeout=5),
        provider=provider,
        manager=manager,
        router=cast(Any, MockFetcherRouter()),
        is_root=False,
        display=_display("child-empty", False),
    )
    root_agent._child_agents["child-empty"] = child_agent
    manager.register_child("child-empty", "root-merge-4", None)

    n = root_agent._merge_child_results("child-empty", child_agent)
    assert n == 0
    assert root_state.extracted_data == []
    merge_steps = [s for s in root_state.history if s.action == "__child_merge__"]
    assert len(merge_steps) == 0
