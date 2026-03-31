import inspect

from agentic_crawler.agent.state import AgentState
from agentic_crawler.agent.prompt_builder import build_messages, build_plan_messages, SYSTEM_PROMPT
from agentic_crawler.agent.tools import get_action_registry, get_tool_schemas
from agentic_crawler.agent.loop import run_agent
from agentic_crawler.config import Settings
from agentic_crawler.agent.manager import AgentManager


def test_agent_state_basics() -> None:
    state = AgentState(goal="Test goal", max_steps=10)
    assert state.step_count == 0
    assert not state.done

    state.add_step(
        "navigate", {"url": "https://example.com"}, "Navigated to example.com", success=True
    )
    assert state.step_count == 1
    assert len(state.history) == 1
    assert state.consecutive_errors == 0

    state.add_step("click", {"selector": "#btn"}, "Click failed", success=False)
    assert state.consecutive_errors == 1

    state.mark_done("Test done")
    assert state.done
    assert state.done_reason == "Test done"


def test_build_messages() -> None:
    state = AgentState(goal="Scrape prices", plan=["Navigate", "Extract"])
    messages = build_messages(state)

    assert messages[0]["role"] == "system"
    assert "autonomous web crawling agent" in messages[0]["content"]
    assert any("Scrape prices" in m["content"] for m in messages if m["role"] == "user")


def test_system_prompt_mentions_fork() -> None:
    assert "fork" in SYSTEM_PROMPT.lower()
    assert "Parallel Exploration" in SYSTEM_PROMPT


def test_system_prompt_mentions_wait_for_subagents() -> None:
    assert "wait_for_subagents" in SYSTEM_PROMPT


def test_build_messages_includes_fork_status_when_children_active() -> None:
    state = AgentState(goal="Scrape prices", plan=["Navigate", "Extract"])
    state.page_summary = "Test page content"
    active_children = [
        {"id": "fork-1", "sub_goal": "Extract product names"},
        {"id": "fork-2", "sub_goal": "Extract prices"},
    ]
    messages = build_messages(state, active_children=active_children)

    user_messages = [m["content"] for m in messages if m["role"] == "user"]
    combined_content = "\n".join(str(m) for m in user_messages)
    assert "Active Subagents (2)" in combined_content
    assert "fork-1" in combined_content
    assert "Extract product names" in combined_content
    assert "fork-2" in combined_content
    assert "Extract prices" in combined_content


def test_build_plan_messages() -> None:
    messages = build_plan_messages("Find all products on example.com")
    assert len(messages) == 2
    assert messages[0]["role"] == "system"
    assert "planning" in messages[0]["content"].lower()


def test_tool_schemas() -> None:
    schemas = get_tool_schemas()
    names = {s["name"] for s in schemas}
    assert "navigate" in names
    assert "click" in names
    assert "fill_form" in names
    assert "extract_data" in names
    assert "done" in names


def test_action_registry() -> None:
    registry = get_action_registry()
    assert "navigate" in registry
    assert "click" in registry
    assert "fill_form" in registry
    assert "extract_data" in registry


def test_run_agent_still_works_as_function() -> None:
    """run_agent() still works as an async function with same signature."""
    sig = inspect.signature(run_agent)
    assert "goal" in sig.parameters
    assert "settings" in sig.parameters
    assert "verbose" in sig.parameters
    import asyncio

    assert asyncio.iscoroutinefunction(run_agent)


def test_cli_passes_fork_settings() -> None:
    """Verify fork settings are accessible and have correct types."""
    settings = Settings()
    assert isinstance(settings.max_concurrent_per_parent, int)
    assert isinstance(settings.max_fork_depth, int)
    assert isinstance(settings.max_total_agents, int)
    assert isinstance(settings.fork_child_max_steps, int)
    assert isinstance(settings.fork_wait_timeout, int)
    manager = AgentManager(
        max_concurrent_per_parent=settings.max_concurrent_per_parent,
        max_depth=settings.max_fork_depth,
        max_total=settings.max_total_agents,
    )
    assert manager._max_concurrent_per_parent == 5
    assert manager._max_depth == 3
    assert manager._max_total == 10
