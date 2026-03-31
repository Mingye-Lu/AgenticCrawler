from __future__ import annotations

import copy
from dataclasses import dataclass, field
from typing import Any


@dataclass
class StepRecord:
    action: str
    params: dict[str, Any]
    observation: str
    success: bool
    tool_call_id: str | None = None


@dataclass
class ChildBlock:
    """A block of extracted data from a single subagent."""
    child_id: str
    sub_goal: str
    items: list[Any]


@dataclass
class AgentState:
    goal: str
    current_url: str | None = None
    page_summary: str | None = None
    current_html: str | None = None
    plan: list[str] = field(default_factory=list)
    history: list[StepRecord] = field(default_factory=list)
    extracted_data: list[Any] = field(default_factory=list)
    child_blocks: list[ChildBlock] = field(default_factory=list)
    step_count: int = 0
    max_steps: int = 50
    errors: list[str] = field(default_factory=list)
    done: bool = False
    done_reason: str = ""
    total_tokens: int = 0

    @property
    def all_data(self) -> list[Any]:
        """Return all extracted data (own + children) as a flat list."""
        result = list(self.extracted_data)
        for block in self.child_blocks:
            result.extend(block.items)
        return result

    def add_step(
        self,
        action: str,
        params: dict[str, Any],
        observation: str,
        success: bool,
        tool_call_id: str | None = None,
    ) -> None:
        self.history.append(
            StepRecord(
                action=action,
                params=params,
                observation=observation,
                success=success,
                tool_call_id=tool_call_id,
            )
        )
        self.step_count += 1
        if not success:
            self.errors.append(f"Step {self.step_count}: {observation}")

    def mark_done(self, reason: str = "Goal completed") -> None:
        self.done = True
        self.done_reason = reason

    def add_text_response(self, text: str) -> None:
        """Record a text-only response (no tool calls) so it appears in history."""
        self.history.append(
            StepRecord(
                action="__text_response__",
                params={},
                observation=text,
                success=True,
            )
        )

    @property
    def consecutive_errors(self) -> int:
        count = 0
        for step in reversed(self.history):
            if not step.success:
                count += 1
            else:
                break
        return count

    def fork(self, sub_goal: str, url: str | None = None) -> AgentState:
        """Create a child state for a sub-goal with deep-copied history and plan.

        Args:
            sub_goal: The goal for the child state
            url: Optional URL for the child state; inherits parent's current_url if not provided

        Returns:
            A new AgentState with copied history/plan and reset transient fields
        """
        return AgentState(
            goal=sub_goal,
            current_url=url if url is not None else self.current_url,
            page_summary=None,
            current_html=None,
            plan=copy.deepcopy(self.plan),
            history=copy.deepcopy(self.history),
            extracted_data=[],
            step_count=0,
            max_steps=self.max_steps,
            errors=[],
            done=False,
            done_reason="",
            total_tokens=0,
        )
