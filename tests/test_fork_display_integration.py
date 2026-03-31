from __future__ import annotations

import asyncio
import io
from typing import Any
from unittest.mock import patch

from rich.console import Console

from agentic_crawler.agent.crawl_agent import CrawlAgent
from agentic_crawler.agent.display import ConsoleDisplay, LiveDashboard
from agentic_crawler.agent.manager import AgentManager
from agentic_crawler.agent.state import AgentState
from agentic_crawler.config import Settings
from agentic_crawler.fetcher.base import FetchResult
from agentic_crawler.llm.base import LLMResponse, ToolCall


class MockFetcherRouter:
    def __init__(self) -> None:
        self._using_browser = False
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
        return FetchResult(url=url, status_code=200, html="<html><body>test</body></html>")

    async def close(self) -> None:
        self.closed = True


class GoalDispatchProvider:
    def __init__(self, response_map: dict[str, list[LLMResponse]]) -> None:
        self.response_map = {k: list(v) for k, v in response_map.items()}
        self.call_counts: dict[str, int] = {k: 0 for k in response_map}

    async def complete(
        self,
        messages: list[dict[str, Any]],
        tools: list[dict[str, Any]] | None = None,
        temperature: float = 0.0,
        on_thinking: Any = None,
    ) -> LLMResponse:
        del tools, temperature, on_thinking
        goal = self._extract_goal(messages)
        responses = self.response_map.get(goal, [])
        idx = self.call_counts.get(goal, 0)
        self.call_counts[goal] = idx + 1
        if idx < len(responses):
            return responses[idx]
        return LLMResponse(
            tool_calls=[
                ToolCall(id="done", name="done", arguments={"summary": f"done:{goal[:15]}"})
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


def _display(agent_id: str) -> ConsoleDisplay:
    return ConsoleDisplay(
        console=Console(file=io.StringIO(), force_terminal=False),
        agent_id=agent_id,
        is_root=False,
    )


def _agent(
    *,
    root_id: str,
    root_goal: str,
    provider: GoalDispatchProvider,
    settings: Settings,
    manager: AgentManager,
) -> CrawlAgent:
    state = AgentState(goal=root_goal, max_steps=settings.max_steps)
    manager.register_root(root_id)
    return CrawlAgent(
        agent_id=root_id,
        state=state,
        settings=settings,
        provider=provider,
        manager=manager,
        router=None,
        is_root=False,
        display=_display(root_id),
    )


async def test_first_fork_swaps_display_and_replays_root_step_count() -> None:
    root_goal = "root display swap"
    child_goal = "child display swap"
    provider = GoalDispatchProvider({})
    settings = _settings()
    manager = _manager(settings)

    with patch("agentic_crawler.agent.crawl_agent.FetcherRouter") as router_cls:
        router_cls.return_value = MockFetcherRouter()
        agent = _agent(
            root_id="root-disp-step",
            root_goal=root_goal,
            provider=provider,
            settings=settings,
            manager=manager,
        )

        agent.state.step_count = 3
        assert isinstance(agent.display, ConsoleDisplay)

        child_id = await agent.fork(child_goal)

        assert child_id.startswith("fork-")
        assert isinstance(agent.display, LiveDashboard)
        assert "root-disp-step" in agent.display._agents
        assert agent.display._agents["root-disp-step"].current_step == 3

        await asyncio.gather(*manager.get_child_tasks("root-disp-step"), return_exceptions=True)


async def test_all_children_share_same_dashboard_instance() -> None:
    root_goal = "root shared dashboard"
    child_goal_1 = "child shared 1"
    child_goal_2 = "child shared 2"
    provider = GoalDispatchProvider(
        {
            root_goal: [
                LLMResponse(
                    tool_calls=[
                        ToolCall(id="f1", name="fork", arguments={"sub_goal": child_goal_1})
                    ]
                ),
                LLMResponse(
                    tool_calls=[
                        ToolCall(id="f2", name="fork", arguments={"sub_goal": child_goal_2})
                    ]
                ),
                LLMResponse(
                    tool_calls=[ToolCall(id="d1", name="done", arguments={"summary": "root done"})]
                ),
            ]
        }
    )
    settings = _settings()
    manager = _manager(settings)

    with patch("agentic_crawler.agent.crawl_agent.FetcherRouter") as router_cls:
        router_cls.return_value = MockFetcherRouter()
        agent = _agent(
            root_id="root-shared-dash",
            root_goal=root_goal,
            provider=provider,
            settings=settings,
            manager=manager,
        )
        await agent.run()

    assert isinstance(agent.display, LiveDashboard)
    assert len(agent._child_agents) == 2
    assert all(child.display is agent.display for child in agent._child_agents.values())
    assert agent.display._total_registered == 3


async def test_dashboard_stops_after_all_agents_complete() -> None:
    root_goal = "root clean shutdown"
    child_goal = "child clean shutdown"
    provider = GoalDispatchProvider(
        {
            root_goal: [
                LLMResponse(
                    tool_calls=[ToolCall(id="f1", name="fork", arguments={"sub_goal": child_goal})]
                ),
                LLMResponse(
                    tool_calls=[ToolCall(id="d1", name="done", arguments={"summary": "root done"})]
                ),
            ]
        }
    )
    settings = _settings()
    manager = _manager(settings)

    with patch("agentic_crawler.agent.crawl_agent.FetcherRouter") as router_cls:
        router_cls.return_value = MockFetcherRouter()
        agent = _agent(
            root_id="root-clean-stop",
            root_goal=root_goal,
            provider=provider,
            settings=settings,
            manager=manager,
        )
        await agent.run()

    assert isinstance(agent.display, LiveDashboard)
    assert agent.display._live is None
    assert agent.display._total_registered == 2
    assert agent.display._total_done == 2


async def test_second_fork_reuses_existing_dashboard() -> None:
    root_goal = "root manual double fork"
    provider = GoalDispatchProvider({})
    settings = _settings()
    manager = _manager(settings)

    with patch("agentic_crawler.agent.crawl_agent.FetcherRouter") as router_cls:
        router_cls.return_value = MockFetcherRouter()
        agent = _agent(
            root_id="root-second-fork",
            root_goal=root_goal,
            provider=provider,
            settings=settings,
            manager=manager,
        )

        first_child = await agent.fork("child one")
        assert first_child.startswith("fork-")
        first_dashboard = agent.display

        second_child = await agent.fork("child two")
        assert second_child.startswith("fork-")

        assert isinstance(agent.display, LiveDashboard)
        assert agent.display is first_dashboard
        assert agent.display._total_registered == 3

        await asyncio.gather(*manager.get_child_tasks("root-second-fork"), return_exceptions=True)
