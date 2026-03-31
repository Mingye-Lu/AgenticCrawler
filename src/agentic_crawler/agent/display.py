from __future__ import annotations

from typing import Any, Protocol

from rich.console import Console
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
