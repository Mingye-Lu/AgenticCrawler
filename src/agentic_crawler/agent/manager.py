"""AgentManager for hierarchical fork lifecycle and limit enforcement."""

from __future__ import annotations

import asyncio
from dataclasses import dataclass


class ForkLimitError(Exception):
    """Raised when a fork operation violates configured limits."""

    pass


@dataclass
class AgentInfo:
    """Information about a registered agent in the hierarchy."""

    agent_id: str
    parent_id: str | None
    depth: int
    task: asyncio.Task[object] | None = None
    status: str = "active"


class AgentManager:
    """Manages hierarchical fork lifecycle and enforces limits.

    Tracks agents in a tree structure and enforces three limits:
    - max_concurrent_per_parent: max children per parent
    - max_depth: max depth in tree (root=0)
    - max_total: max total agents across entire tree
    """

    def __init__(
        self,
        max_concurrent_per_parent: int,
        max_depth: int,
        max_total: int,
    ) -> None:
        """Initialize AgentManager with fork limits.

        Args:
            max_concurrent_per_parent: Maximum concurrent children per parent
            max_depth: Maximum depth in agent tree (root=0)
            max_total: Maximum total agents allowed
        """
        self._max_concurrent_per_parent = max_concurrent_per_parent
        self._max_depth = max_depth
        self._max_total = max_total
        self._agents: dict[str, AgentInfo] = {}
        self._semaphore = asyncio.Semaphore(max_total)

    def register_root(self, agent_id: str) -> None:
        """Register a root agent (depth 0).

        Args:
            agent_id: Unique identifier for the root agent
        """
        self._agents[agent_id] = AgentInfo(
            agent_id=agent_id,
            parent_id=None,
            depth=0,
            task=None,
            status="active",
        )

    def can_fork(self, parent_id: str) -> bool:
        """Check if a parent can fork a new child.

        Returns True if all three limits would be satisfied:
        - Parent doesn't exceed max_concurrent_per_parent
        - Child depth wouldn't exceed max_depth
        - Total agents wouldn't exceed max_total

        Args:
            parent_id: ID of the parent agent

        Returns:
            True if fork is allowed, False otherwise
        """
        if parent_id not in self._agents:
            return False

        parent = self._agents[parent_id]

        # Check concurrent limit
        active_children = self.get_active_children(parent_id)
        if len(active_children) >= self._max_concurrent_per_parent:
            return False

        # Check depth limit
        if parent.depth + 1 > self._max_depth:
            return False

        # Check total limit
        if len(self._agents) >= self._max_total:
            return False

        return True

    def register_child(
        self,
        child_id: str,
        parent_id: str,
        task: asyncio.Task[object] | None,
    ) -> None:
        """Register a child agent under a parent.

        Raises ForkLimitError if any limit is violated.

        Args:
            child_id: Unique identifier for the child agent
            parent_id: ID of the parent agent
            task: Optional asyncio.Task for the child

        Raises:
            ForkLimitError: If any fork limit is exceeded
        """
        if not self.can_fork(parent_id):
            raise ForkLimitError(
                f"Cannot fork child {child_id} under parent {parent_id}: fork limits exceeded"
            )

        parent = self._agents[parent_id]
        child_depth = parent.depth + 1

        self._agents[child_id] = AgentInfo(
            agent_id=child_id,
            parent_id=parent_id,
            depth=child_depth,
            task=task,
            status="active",
        )

    def get_children(self, parent_id: str) -> list[AgentInfo]:
        """Get all children of a parent (including done ones).

        Args:
            parent_id: ID of the parent agent

        Returns:
            List of all child AgentInfo objects
        """
        return [agent for agent in self._agents.values() if agent.parent_id == parent_id]

    def get_active_children(self, parent_id: str) -> list[AgentInfo]:
        """Get only active (non-done) children of a parent.

        Args:
            parent_id: ID of the parent agent

        Returns:
            List of active child AgentInfo objects
        """
        return [
            agent
            for agent in self._agents.values()
            if agent.parent_id == parent_id and agent.status == "active"
        ]

    def get_depth(self, agent_id: str) -> int:
        """Get the depth of an agent in the tree.

        Args:
            agent_id: ID of the agent

        Returns:
            Depth (root=0, child=1, grandchild=2, etc.)
        """
        return self._agents[agent_id].depth

    def mark_done(self, agent_id: str) -> None:
        """Mark an agent as done.

        Args:
            agent_id: ID of the agent to mark as done
        """
        if agent_id in self._agents:
            self._agents[agent_id].status = "done"

    def get_child_tasks(self, parent_id: str) -> list[asyncio.Task[object]]:
        """Get asyncio.Task list for active children.

        Args:
            parent_id: ID of the parent agent

        Returns:
            List of asyncio.Task objects for active children
        """
        tasks = []
        for child in self.get_active_children(parent_id):
            if child.task is not None:
                tasks.append(child.task)
        return tasks
