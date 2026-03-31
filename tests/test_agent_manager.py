"""Tests for AgentManager hierarchical fork lifecycle and limit enforcement."""

from __future__ import annotations

import pytest

from agentic_crawler.agent.manager import AgentManager, ForkLimitError


@pytest.fixture
def manager() -> AgentManager:
    """Create a fresh AgentManager with reasonable limits."""
    return AgentManager(
        max_concurrent_per_parent=3,
        max_depth=3,
        max_total=10,
    )


def test_register_root_agent(manager: AgentManager) -> None:
    """Test registering a root agent."""
    manager.register_root("root-1")

    # Verify root is tracked
    assert manager.get_depth("root-1") == 0
    children = manager.get_children("root-1")
    assert len(children) == 0


def test_can_fork_returns_true_within_limits(manager: AgentManager) -> None:
    """Test can_fork returns True when all limits are satisfied."""
    manager.register_root("root-1")

    # Should be able to fork from root
    assert manager.can_fork("root-1") is True


def test_max_concurrent_per_parent_enforced(manager: AgentManager) -> None:
    """Test that max_concurrent_per_parent limit is enforced."""
    manager.register_root("root-1")

    # Register max_concurrent_per_parent children
    for i in range(manager._max_concurrent_per_parent):
        child_id = f"child-{i}"
        manager.register_child(child_id, "root-1", None)

    # Next child should raise ForkLimitError
    with pytest.raises(ForkLimitError):
        manager.register_child("child-overflow", "root-1", None)


def test_max_depth_enforced(manager: AgentManager) -> None:
    """Test that max_depth limit is enforced."""
    # Create a chain: root -> child -> grandchild -> great-grandchild
    manager.register_root("root-1")
    manager.register_child("child-1", "root-1", None)
    manager.register_child("grandchild-1", "child-1", None)
    manager.register_child("great-grandchild-1", "grandchild-1", None)

    # At max_depth=3, depth 4 should fail
    with pytest.raises(ForkLimitError):
        manager.register_child("great-great-grandchild-1", "great-grandchild-1", None)


def test_max_total_agents_enforced(manager: AgentManager) -> None:
    """Test that max_total agents limit is enforced."""
    # Create multiple root agents to reach max_total
    for i in range(manager._max_total):
        manager.register_root(f"root-{i}")

    # Next child should raise ForkLimitError (total limit exceeded)
    manager.register_root("extra-root")
    with pytest.raises(ForkLimitError):
        manager.register_child("child-overflow", "extra-root", None)


def test_get_children(manager: AgentManager) -> None:
    """Test get_children returns all children including done ones."""
    manager.register_root("parent-1")

    # Register 3 children
    for i in range(3):
        child_id = f"child-{i}"
        manager.register_child(child_id, "parent-1", None)

    # Mark one as done
    manager.mark_done("child-0")

    # get_children should return all 3
    children = manager.get_children("parent-1")
    assert len(children) == 3
    assert all(child.parent_id == "parent-1" for child in children)


def test_get_depth(manager: AgentManager) -> None:
    """Test get_depth returns correct depth in tree."""
    manager.register_root("root-1")
    manager.register_child("child-1", "root-1", None)
    manager.register_child("grandchild-1", "child-1", None)

    assert manager.get_depth("root-1") == 0
    assert manager.get_depth("child-1") == 1
    assert manager.get_depth("grandchild-1") == 2


def test_mark_agent_done(manager: AgentManager) -> None:
    """Test marking an agent as done."""
    manager.register_root("root-1")
    manager.register_child("child-1", "root-1", None)

    # Initially active
    agent_info = manager._agents["child-1"]
    assert agent_info.status == "active"

    # Mark as done
    manager.mark_done("child-1")

    # Should be done
    agent_info = manager._agents["child-1"]
    assert agent_info.status == "done"


def test_get_active_children(manager: AgentManager) -> None:
    """Test get_active_children returns only non-done children."""
    manager.register_root("parent-1")

    # Register 3 children
    for i in range(3):
        child_id = f"child-{i}"
        manager.register_child(child_id, "parent-1", None)

    # Mark one as done
    manager.mark_done("child-0")

    # get_active_children should return only 2
    active = manager.get_active_children("parent-1")
    assert len(active) == 2
    assert all(child.status == "active" for child in active)
    assert all(child.agent_id != "child-0" for child in active)
