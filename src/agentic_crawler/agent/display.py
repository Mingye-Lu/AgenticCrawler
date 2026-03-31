from __future__ import annotations

import re
import threading
from collections import deque
from dataclasses import dataclass, field
from typing import Any, Protocol

from rich.console import Console, Group
from rich.live import Live
from rich.panel import Panel


class AgentDisplay(Protocol):
    def print_panel(self, agent_id: str, title: str, content: str, style: str) -> None: ...
    def log_step(
        self, agent_id: str, step_num: int, timestamp: str, action: str, params_str: str
    ) -> None: ...
    def log_result(self, agent_id: str, status: str, observation: str | None) -> None: ...
    def log_message(self, agent_id: str, msg: str) -> None: ...
    def set_thinking(self, agent_id: str, thinking: bool) -> None: ...
    def stream_thinking_chunk(self, agent_id: str, chunk: str) -> None: ...
    def print_final_output(self, renderable: Any) -> None: ...
    def register_agent(
        self, agent_id: str, goal: str, parent_id: str | None, max_steps: int
    ) -> None: ...
    def mark_agent_done(self, agent_id: str) -> None: ...
    def get_console(self) -> Console: ...


class ConsoleDisplay:
    def __init__(
        self,
        console: Console,
        verbose: bool = False,
        agent_id: str = "",
        is_root: bool = True,
    ) -> None:
        self.console = console
        self.verbose = verbose
        self._agent_id = agent_id
        self._thinking_active = False

        if is_root:
            self._prefix = "[bold dim][root][/bold dim]"
        else:
            short_id = agent_id[:6] if len(agent_id) >= 6 else agent_id
            self._prefix = f"[bold dim][{short_id}][/bold dim]"

    def print_panel(self, agent_id: str, title: str, content: str, style: str) -> None:
        if style:
            self.console.print(Panel(content, title=self._prefix, style=style))
        else:
            self.console.print(Panel(content, title=self._prefix))

    def log_step(
        self, agent_id: str, step_num: int, timestamp: str, action: str, params_str: str
    ) -> None:
        step_label = f"[Step {step_num}]"
        self.console.print(
            f"{self._prefix}   {step_label} [dim]{timestamp}[/dim] {action}({params_str})"
        )

    def log_result(self, agent_id: str, status: str, observation: str | None) -> None:
        if self.verbose and observation is not None:
            self.console.print(f"    {status} {observation}")
        else:
            self.console.print(f"    {status}")

    def log_message(self, agent_id: str, msg: str) -> None:
        self.console.print(f"{self._prefix} {msg}")

    def set_thinking(self, agent_id: str, thinking: bool) -> None:
        """Start or stop the thinking indicator.

        * ``True``  — prints the "Thinking..." header line (once).
        * ``False`` — writes a trailing newline to ``console.file`` and clears the flag.
        """
        if thinking and not self._thinking_active:
            self.console.print(f"{self._prefix} [dim italic]  Thinking...[/dim italic]")
            self._thinking_active = True
        elif not thinking and self._thinking_active:
            self.console.file.write("\n")
            self.console.file.flush()
            self._thinking_active = False

    def stream_thinking_chunk(self, agent_id: str, chunk: str) -> None:
        self.console.file.write(chunk)
        self.console.file.flush()

    def print_final_output(self, renderable: Any) -> None:
        self.console.print(renderable)

    def register_agent(
        self, agent_id: str, goal: str, parent_id: str | None, max_steps: int
    ) -> None:
        pass

    def mark_agent_done(self, agent_id: str) -> None:
        pass

    def get_console(self) -> Console:
        return self.console


# ── LiveDashboard (Rich Live TUI for forked agents) ────────────────


@dataclass
class _AgentPanelState:
    agent_id: str
    goal: str
    parent_id: str | None
    max_steps: int
    current_step: int = 0
    status: str = "Running"
    last_steps: deque[str] = field(default_factory=lambda: deque(maxlen=5))


class LiveDashboard:
    """Multi-agent Rich Live dashboard — one :class:`Panel` per agent."""

    def __init__(self, console: Console, verbose: bool = False) -> None:
        self._console = console
        self.verbose = verbose
        self._agents: dict[str, _AgentPanelState] = {}
        self._live: Live | None = None
        self._lock = threading.Lock()
        self._pending_output: list[Any] = []
        self._total_registered: int = 0
        self._total_done: int = 0

    # ── lifecycle ───────────────────────────────────────────────

    def register_agent(
        self, agent_id: str, goal: str, parent_id: str | None, max_steps: int
    ) -> None:
        self._agents[agent_id] = _AgentPanelState(
            agent_id=agent_id,
            goal=goal,
            parent_id=parent_id,
            max_steps=max_steps,
        )
        self._total_registered += 1

    def start(self) -> None:
        """Activate the Live context. Call after registering initial agents."""
        self._live = Live(
            self._build_renderable(),
            console=self._console,
            refresh_per_second=4,
            transient=False,
        )
        self._live.start()

    def stop(self) -> None:
        """Stop the Live context and flush buffered output."""
        if self._live is not None:
            self._live.stop()
            self._live = None
        for renderable in self._pending_output:
            self._console.print(renderable)
        self._pending_output.clear()

    # ── protocol methods ────────────────────────────────────────

    def log_step(
        self, agent_id: str, step_num: int, timestamp: str, action: str, params_str: str
    ) -> None:
        if agent_id not in self._agents:
            return
        state = self._agents[agent_id]
        state.current_step = step_num
        state.status = "Running"
        step_str = f"[Step {step_num}] {timestamp} {action}({params_str})"
        state.last_steps.append(step_str)
        self._refresh()

    def log_result(self, agent_id: str, status: str, observation: str | None) -> None:
        if agent_id not in self._agents:
            return
        state = self._agents[agent_id]
        if state.last_steps:
            last = state.last_steps[-1]
            status_plain = "OK" if "green" in status else "FAIL"
            state.last_steps[-1] = f"{last} → {status_plain}"
        self._refresh()

    def log_message(self, agent_id: str, msg: str) -> None:
        if agent_id not in self._agents:
            return
        state = self._agents[agent_id]
        clean_msg = re.sub(r"\[/?[^\]]*\]", "", msg).strip()
        if clean_msg:
            state.last_steps.append(clean_msg[:80])
        self._refresh()

    def set_thinking(self, agent_id: str, thinking: bool) -> None:
        if agent_id not in self._agents:
            return
        state = self._agents[agent_id]
        if thinking:
            state.status = "Thinking..."
        else:
            state.status = f"Running (Step {state.current_step}/{state.max_steps})"
        self._refresh()

    def stream_thinking_chunk(self, agent_id: str, chunk: str) -> None:
        pass

    def print_panel(self, agent_id: str, title: str, content: str, style: str) -> None:
        if agent_id not in self._agents:
            return
        state = self._agents[agent_id]
        if "Planning" in content:
            state.status = "Planning..."
        elif "Executing" in content:
            state.status = f"Running (Step 0/{state.max_steps})"
        elif "Done" in content or not style:
            state.status = "Done"
        self._refresh()

    def print_final_output(self, renderable: Any) -> None:
        self._pending_output.append(renderable)

    def mark_agent_done(self, agent_id: str) -> None:
        if agent_id in self._agents:
            self._agents[agent_id].status = "Done"
            self._refresh()
        self._total_done += 1
        if self._total_done >= self._total_registered and self._total_registered > 0:
            self.stop()

    def get_console(self) -> Console:
        return self._console

    # ── internals ───────────────────────────────────────────────

    def _build_renderable(self) -> Group:
        panels: list[Panel] = []
        for state in self._agents.values():
            short_id = state.agent_id[:6] if len(state.agent_id) >= 6 else state.agent_id
            prefix = "[root]" if state.parent_id is None else f"[{short_id}]"

            goal_truncated = state.goal[:60] + "..." if len(state.goal) > 60 else state.goal
            status_line = f"Status: {state.status}"
            if state.status == "Running":
                status_line = f"Status: Running (Step {state.current_step}/{state.max_steps})"

            log_lines = list(state.last_steps)
            lines = [
                f"[bold]{prefix}[/bold] {goal_truncated}",
                status_line,
                "─" * 40,
                *log_lines,
            ]
            content = "\n".join(lines)
            style = "green" if state.status == "Done" else "blue"
            panels.append(Panel(content, style=style))

        return Group(*panels)

    def _refresh(self) -> None:
        with self._lock:
            if self._live is not None:
                self._live.update(self._build_renderable())
