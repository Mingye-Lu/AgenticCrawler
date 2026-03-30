import pytest

from agentic_crawler.agent.state import AgentState
from agentic_crawler.agent.prompt_builder import build_messages, build_plan_messages
from agentic_crawler.agent.tools import get_action_registry, get_tool_schemas


def test_agent_state_basics() -> None:
    state = AgentState(goal="Test goal", max_steps=10)
    assert state.step_count == 0
    assert not state.done

    state.add_step("navigate", {"url": "https://example.com"}, "Navigated to example.com", success=True)
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
