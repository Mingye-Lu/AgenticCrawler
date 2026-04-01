from __future__ import annotations

import re
import threading
from dataclasses import dataclass, field
from typing import Any

from rich.console import Console

from agentic_crawler.tui.permissions import PermissionPolicy
from agentic_crawler.tui.renderer import MarkdownStreamState
from agentic_crawler.tui.tool_display import format_tool_result


@dataclass
class _AgentInfo:
    agent_id: str
    goal: str
    parent_id: str | None
    max_steps: int
    _stream_state: MarkdownStreamState = field(default_factory=MarkdownStreamState)


class ReplDisplay:
    def __init__(self, console: Console, verbose: bool = False) -> None:
        self._console = console
        self.verbose = verbose
        self._agents: dict[str, _AgentInfo] = {}
        self._lock = threading.Lock()
        self._permission_policy = PermissionPolicy()

    def _agent_prefix(self, agent_id: str) -> str:
        short = agent_id[:6] if len(agent_id) >= 6 else agent_id
        return f"[{short}] "

    def _print(self, text: str) -> None:
        with self._lock:
            self._console.print(text, end="", markup=False, highlight=False)

    def register_agent(
        self, agent_id: str, goal: str, parent_id: str | None, max_steps: int
    ) -> None:
        self._agents[agent_id] = _AgentInfo(
            agent_id=agent_id,
            goal=goal,
            parent_id=parent_id,
            max_steps=max_steps,
        )
        self._print(f"\n{self._agent_prefix(agent_id)}Goal: {goal}\n")

    def print_panel(self, agent_id: str, title: str, content: str, style: str) -> None:
        if agent_id not in self._agents:
            return
        prefix = self._agent_prefix(agent_id)
        self._print(f"{prefix}{title}: {content}\n")

    def log_step(
        self,
        agent_id: str,
        step_num: int,
        timestamp: str,
        action: str,
        params_str: str,
    ) -> None:
        if agent_id not in self._agents:
            return
        prefix = self._agent_prefix(agent_id)
        self._print(f"{prefix}▶ Step {step_num}: {action}({params_str})\n")

    def log_result(self, agent_id: str, status: str, observation: str | None) -> None:
        if agent_id not in self._agents:
            return
        success = "green" in status
        last_action = self._agents[agent_id].agent_id
        formatted = format_tool_result(last_action, success, observation)
        self._print(f"{formatted}\n")

    def log_message(self, agent_id: str, msg: str) -> None:
        if agent_id not in self._agents:
            return
        clean_msg = re.sub(r"\[/?[^\]]*\]", "", msg).strip()
        if clean_msg:
            prefix = self._agent_prefix(agent_id)
            self._print(f"{prefix}{clean_msg}\n")

    def set_thinking(self, agent_id: str, thinking: bool) -> None:
        if agent_id not in self._agents:
            return
        if thinking:
            prefix = self._agent_prefix(agent_id)
            self._print(f"{prefix}⟳ Thinking...\n")

    def stream_thinking_chunk(self, agent_id: str, chunk: str) -> None:
        if agent_id not in self._agents:
            return
        rendered = self._agents[agent_id]._stream_state.push(chunk)
        if rendered is not None:
            self._print(rendered)

    def stream_text_delta(self, agent_id: str, chunk: str) -> None:
        if agent_id not in self._agents:
            return
        rendered = self._agents[agent_id]._stream_state.push(chunk)
        if rendered is not None:
            self._print(rendered)

    def print_final_output(self, renderable: Any) -> None:
        with self._lock:
            self._console.print(renderable)

    def mark_agent_done(self, agent_id: str) -> None:
        if agent_id in self._agents:
            remaining = self._agents[agent_id]._stream_state.flush()
            if remaining is not None:
                self._print(remaining)
        prefix = self._agent_prefix(agent_id)
        self._print(f"{prefix}✓ Done\n")

    def get_console(self) -> Console:
        return self._console
