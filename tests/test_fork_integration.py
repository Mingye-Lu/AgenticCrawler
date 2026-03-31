from __future__ import annotations

import asyncio
import io
from collections.abc import Callable
from typing import Any
from unittest.mock import patch

from rich.console import Console

from agentic_crawler.agent.crawl_agent import CrawlAgent
from agentic_crawler.agent.display import LiveDashboard
from agentic_crawler.agent.manager import AgentManager
from agentic_crawler.agent.state import AgentState
from agentic_crawler.config import Settings
from agentic_crawler.fetcher.base import FetchResult
from agentic_crawler.llm.base import LLMResponse, ToolCall


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


class GoalDispatchMockProvider:
    def __init__(
        self,
        response_map: dict[str, list[LLMResponse | Exception]],
        delay_map: dict[str, list[float]] | None = None,
    ) -> None:
        self.response_map = {k: list(v) for k, v in response_map.items()}
        self.delay_map = {k: list(v) for k, v in (delay_map or {}).items()}
        self.call_counts: dict[str, int] = {k: 0 for k in response_map}
        self.messages_log: list[list[dict[str, Any]]] = []
        self._lock = asyncio.Lock()

    async def complete(
        self,
        messages: list[dict[str, Any]],
        tools: list[dict[str, Any]] | None = None,
        temperature: float = 0.0,
        on_thinking: Callable[[str], None] | None = None,
    ) -> LLMResponse:
        del tools, temperature, on_thinking
        self.messages_log.append(messages)

        async with self._lock:
            goal = self._extract_goal(messages)
            responses = self.response_map.get(goal, [])
            call_count = self.call_counts.get(goal, 0)
            self.call_counts[goal] = call_count + 1
            response = responses[call_count] if call_count < len(responses) else None
            delays = self.delay_map.get(goal, [])
            delay = delays[call_count] if call_count < len(delays) else 0.0

        if delay > 0:
            await asyncio.sleep(delay)

        if isinstance(response, Exception):
            raise response

        if response is not None:
            return response

        return LLMResponse(
            tool_calls=[
                ToolCall(
                    id=f"done_{goal[:6]}",
                    name="done",
                    arguments={"summary": f"done: {goal[:20]}"},
                )
            ]
        )

    def _extract_goal(self, messages: list[dict[str, Any]]) -> str:
        for msg in messages:
            if msg.get("role") != "user":
                continue

            content = msg.get("content", "")
            if isinstance(content, str) and content.startswith("## Goal\n"):
                lines = content.split("\n")
                if len(lines) > 1:
                    return lines[1].strip()
        return "unknown"

    async def close(self) -> None:
        pass


def _settings(**overrides: Any) -> Settings:
    base: dict[str, Any] = {
        "max_steps": 10,
        "fork_child_max_steps": 5,
        "fork_wait_timeout": 5,
        "max_concurrent_per_parent": 5,
        "max_fork_depth": 3,
        "max_total_agents": 10,
    }
    base.update(overrides)
    return Settings(**base)


def _manager(settings: Settings) -> AgentManager:
    return AgentManager(
        max_concurrent_per_parent=settings.max_concurrent_per_parent,
        max_depth=settings.max_fork_depth,
        max_total=settings.max_total_agents,
    )


def _display(agent_id: str, is_root: bool = False) -> LiveDashboard:
    dash = LiveDashboard(
        console=Console(file=io.StringIO(), force_terminal=False),
    )
    dash.register_agent(agent_id, "test", None if is_root else "root", 50)
    return dash


def _root_agent(
    *,
    root_id: str,
    root_goal: str,
    provider: GoalDispatchMockProvider,
    settings: Settings,
    manager: AgentManager,
) -> CrawlAgent:
    state = AgentState(goal=root_goal, max_steps=settings.max_steps)
    manager.register_root(root_id)
    display = _display(root_id, is_root=False)
    display.register_agent(root_id, root_goal, None, settings.max_steps)
    return CrawlAgent(
        agent_id=root_id,
        state=state,
        settings=settings,
        provider=provider,
        manager=manager,
        router=None,
        is_root=False,
        display=display,
    )


async def test_single_fork_e2e() -> None:
    root_goal = "root single fork"
    child_goal = "child single fork"
    provider = GoalDispatchMockProvider(
        {
            root_goal: [
                LLMResponse(
                    tool_calls=[ToolCall(id="f1", name="fork", arguments={"sub_goal": child_goal})]
                ),
                LLMResponse(
                    tool_calls=[ToolCall(id="d1", name="done", arguments={"summary": "root done"})]
                ),
            ],
            child_goal: [
                LLMResponse(
                    tool_calls=[
                        ToolCall(id="cd1", name="done", arguments={"summary": "child done"})
                    ]
                )
            ],
        }
    )
    settings = _settings()
    manager = _manager(settings)

    with patch("agentic_crawler.agent.crawl_agent.FetcherRouter") as router_cls:
        router_cls.return_value = MockFetcherRouter()
        agent = _root_agent(
            root_id="root-int-1",
            root_goal=root_goal,
            provider=provider,
            settings=settings,
            manager=manager,
        )
        await agent.run()

    assert agent.state.done is True
    assert agent.state.done_reason == "root done"
    children = manager.get_children("root-int-1")
    assert len(children) == 1
    assert children[0].status == "done"


async def test_child_data_merges_to_parent() -> None:
    root_goal = "root merge"
    child_goal = "child extraction"
    provider = GoalDispatchMockProvider(
        {
            root_goal: [
                LLMResponse(
                    tool_calls=[ToolCall(id="f1", name="fork", arguments={"sub_goal": child_goal})]
                ),
                LLMResponse(
                    tool_calls=[ToolCall(id="w1", name="wait_for_subagents", arguments={})]
                ),
                LLMResponse(
                    tool_calls=[ToolCall(id="d1", name="done", arguments={"summary": "done"})]
                ),
            ],
            child_goal: [
                LLMResponse(
                    tool_calls=[
                        ToolCall(
                            id="e1",
                            name="extract_data",
                            arguments={
                                "instruction": "extract item",
                                "data": {"title": "child-item", "price": "$10"},
                            },
                        )
                    ]
                ),
                LLMResponse(
                    tool_calls=[
                        ToolCall(id="cd1", name="done", arguments={"summary": "child done"})
                    ]
                ),
            ],
        }
    )
    settings = _settings()
    manager = _manager(settings)

    with patch("agentic_crawler.agent.crawl_agent.FetcherRouter") as router_cls:
        router_cls.return_value = MockFetcherRouter()
        agent = _root_agent(
            root_id="root-int-2",
            root_goal=root_goal,
            provider=provider,
            settings=settings,
            manager=manager,
        )
        await agent.run()

    assert agent.state.done is True
    assert len(agent.state.child_blocks) == 1
    assert agent.state.child_blocks[0].items == [{"title": "child-item", "price": "$10"}]
    assert agent.state.all_data == [{"title": "child-item", "price": "$10"}]
    merge_steps = [s for s in agent.state.history if s.action == "__child_merge__"]
    assert len(merge_steps) == 1
    assert "extracted 1 item(s)" in merge_steps[0].observation


async def test_fork_limit_graceful_degradation() -> None:
    root_goal = "root limit"
    child_goal = "child first"
    provider = GoalDispatchMockProvider(
        {
            root_goal: [
                LLMResponse(
                    tool_calls=[
                        ToolCall(id="f1", name="fork", arguments={"sub_goal": child_goal}),
                        ToolCall(id="f2", name="fork", arguments={"sub_goal": "child second"}),
                        ToolCall(id="d1", name="done", arguments={"summary": "root done"}),
                    ]
                )
            ],
            child_goal: [
                LLMResponse(
                    tool_calls=[
                        ToolCall(id="cd1", name="done", arguments={"summary": "child done"})
                    ]
                )
            ],
        }
    )
    settings = _settings(max_concurrent_per_parent=1)
    manager = _manager(settings)

    with patch("agentic_crawler.agent.crawl_agent.FetcherRouter") as router_cls:
        router_cls.return_value = MockFetcherRouter()
        agent = _root_agent(
            root_id="root-int-3",
            root_goal=root_goal,
            provider=provider,
            settings=settings,
            manager=manager,
        )
        await agent.run()

    fork_steps = [s for s in agent.state.history if s.action == "fork"]
    assert len(fork_steps) == 2
    assert fork_steps[0].success is True
    assert fork_steps[1].success is False
    assert "Cannot fork" in fork_steps[1].observation


async def test_child_error_does_not_crash_parent() -> None:
    root_goal = "root resilient"
    child_goal = "child crashes"
    provider = GoalDispatchMockProvider(
        {
            root_goal: [
                LLMResponse(
                    tool_calls=[ToolCall(id="f1", name="fork", arguments={"sub_goal": child_goal})]
                ),
                LLMResponse(
                    tool_calls=[
                        ToolCall(id="d1", name="done", arguments={"summary": "root still done"})
                    ]
                ),
            ],
            child_goal: [RuntimeError("child provider crash")],
        }
    )
    settings = _settings()
    manager = _manager(settings)

    with patch("agentic_crawler.agent.crawl_agent.FetcherRouter") as router_cls:
        router_cls.return_value = MockFetcherRouter()
        agent = _root_agent(
            root_id="root-int-4",
            root_goal=root_goal,
            provider=provider,
            settings=settings,
            manager=manager,
        )
        await agent.run()

    assert agent.state.done is True
    assert agent.state.done_reason == "root still done"
    children = manager.get_children("root-int-4")
    assert len(children) == 1
    assert children[0].status == "done"


async def test_recursive_fork() -> None:
    root_goal = "root recursive"
    child_goal = "child recursive"
    grandchild_goal = "grandchild recursive"
    provider = GoalDispatchMockProvider(
        {
            root_goal: [
                LLMResponse(
                    tool_calls=[ToolCall(id="f1", name="fork", arguments={"sub_goal": child_goal})]
                ),
                LLMResponse(
                    tool_calls=[ToolCall(id="w1", name="wait_for_subagents", arguments={})]
                ),
                LLMResponse(
                    tool_calls=[ToolCall(id="d1", name="done", arguments={"summary": "root done"})]
                ),
            ],
            child_goal: [
                LLMResponse(
                    tool_calls=[
                        ToolCall(id="cf1", name="fork", arguments={"sub_goal": grandchild_goal})
                    ]
                ),
                LLMResponse(
                    tool_calls=[
                        ToolCall(id="cd1", name="done", arguments={"summary": "child done"})
                    ]
                ),
            ],
            grandchild_goal: [
                LLMResponse(
                    tool_calls=[
                        ToolCall(id="gd1", name="done", arguments={"summary": "grandchild done"})
                    ]
                )
            ],
        }
    )
    settings = _settings(max_fork_depth=3)
    manager = _manager(settings)

    with patch("agentic_crawler.agent.crawl_agent.FetcherRouter") as router_cls:
        router_cls.return_value = MockFetcherRouter()
        agent = _root_agent(
            root_id="root-int-5",
            root_goal=root_goal,
            provider=provider,
            settings=settings,
            manager=manager,
        )
        await agent.run()

    assert agent.state.done is True
    assert len(manager._agents) == 3
    root_children = manager.get_children("root-int-5")
    assert len(root_children) == 1
    child_id = root_children[0].agent_id
    child_children = manager.get_children(child_id)
    assert len(child_children) == 1
    assert manager.get_depth("root-int-5") == 0
    assert manager.get_depth(child_id) == 1
    assert manager.get_depth(child_children[0].agent_id) == 2


async def test_parent_max_steps_while_children_running() -> None:
    root_goal = "root max steps"
    child_goal = "child slow"
    provider = GoalDispatchMockProvider(
        {
            root_goal: [
                LLMResponse(
                    tool_calls=[ToolCall(id="f1", name="fork", arguments={"sub_goal": child_goal})]
                )
            ],
            child_goal: [
                LLMResponse(
                    tool_calls=[
                        ToolCall(id="cd1", name="done", arguments={"summary": "child done"})
                    ]
                )
            ],
        },
        delay_map={child_goal: [0.2]},
    )
    settings = _settings(max_steps=1, fork_wait_timeout=1)
    manager = _manager(settings)

    with patch("agentic_crawler.agent.crawl_agent.FetcherRouter") as router_cls:
        router_cls.return_value = MockFetcherRouter()
        agent = _root_agent(
            root_id="root-int-6",
            root_goal=root_goal,
            provider=provider,
            settings=settings,
            manager=manager,
        )
        await agent.run()

    assert agent.state.done is False
    assert agent.state.step_count == 1

    child_tasks = manager.get_child_tasks("root-int-6")
    assert len(child_tasks) == 1
    assert child_tasks[0].done() is False

    await asyncio.gather(*child_tasks, return_exceptions=True)
    children = manager.get_children("root-int-6")
    assert len(children) == 1
    assert children[0].status == "done"
