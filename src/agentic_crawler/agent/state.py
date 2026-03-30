from __future__ import annotations

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
class AgentState:
    goal: str
    current_url: str | None = None
    page_summary: str | None = None
    current_html: str | None = None
    plan: list[str] = field(default_factory=list)
    history: list[StepRecord] = field(default_factory=list)
    extracted_data: list[Any] = field(default_factory=list)
    step_count: int = 0
    max_steps: int = 50
    errors: list[str] = field(default_factory=list)
    done: bool = False
    done_reason: str = ""
    total_tokens: int = 0

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
