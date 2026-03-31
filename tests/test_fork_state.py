from __future__ import annotations

import pytest

from agentic_crawler.agent.state import AgentState


@pytest.fixture
def parent_state() -> AgentState:
    """Create a parent state with some history and plan."""
    state = AgentState(
        goal="Find all products",
        current_url="https://example.com",
        page_summary="Product listing page",
        current_html="<html>...</html>",
        plan=["Step 1", "Step 2", "Step 3"],
        step_count=2,
        max_steps=50,
        total_tokens=100,
    )
    # Add some history
    state.add_step(
        action="navigate",
        params={"url": "https://example.com"},
        observation="Navigated to example.com",
        success=True,
    )
    state.add_step(
        action="extract_data",
        params={"selector": ".product"},
        observation="Extracted 5 products",
        success=True,
    )
    return state


def test_fork_copies_history(parent_state: AgentState) -> None:
    """Child history should have same content as parent but be a different list object."""
    child = parent_state.fork(sub_goal="Extract product details")

    # Content should be equal
    assert len(child.history) == len(parent_state.history)
    assert child.history == parent_state.history

    # But should be different objects (deep copy)
    assert child.history is not parent_state.history


def test_fork_sets_sub_goal(parent_state: AgentState) -> None:
    """Child goal should be set to the sub_goal parameter."""
    sub_goal = "Extract product details"
    child = parent_state.fork(sub_goal=sub_goal)

    assert child.goal == sub_goal


def test_fork_inherits_url_when_no_url_given(parent_state: AgentState) -> None:
    """Child should inherit parent's current_url when url parameter is not provided."""
    child = parent_state.fork(sub_goal="Extract product details")

    assert child.current_url == parent_state.current_url
    assert child.current_url == "https://example.com"


def test_fork_overrides_url_when_given(parent_state: AgentState) -> None:
    """Child should use provided url parameter instead of parent's current_url."""
    new_url = "https://example.com/products"
    child = parent_state.fork(sub_goal="Extract product details", url=new_url)

    assert child.current_url == new_url
    assert child.current_url != parent_state.current_url


def test_fork_resets_transient_state(parent_state: AgentState) -> None:
    """Child should reset transient state fields to defaults."""
    child = parent_state.fork(sub_goal="Extract product details")

    # These should be reset
    assert child.current_html is None
    assert child.page_summary is None
    assert child.extracted_data == []
    assert child.errors == []
    assert child.step_count == 0
    assert child.done is False
    assert child.done_reason == ""
    assert child.total_tokens == 0


def test_fork_copies_plan(parent_state: AgentState) -> None:
    """Child plan should have same content as parent."""
    child = parent_state.fork(sub_goal="Extract product details")

    assert child.plan == parent_state.plan
    assert child.plan == ["Step 1", "Step 2", "Step 3"]


def test_fork_history_is_independent(parent_state: AgentState) -> None:
    """Mutating child history should not affect parent history."""
    child = parent_state.fork(sub_goal="Extract product details")

    # Modify child history
    child.add_step(
        action="click",
        params={"selector": ".next"},
        observation="Clicked next button",
        success=True,
    )

    # Parent history should be unchanged
    assert len(parent_state.history) == 2
    assert len(child.history) == 3
