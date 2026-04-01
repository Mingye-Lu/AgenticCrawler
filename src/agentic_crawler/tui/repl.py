from __future__ import annotations

from pathlib import Path

from prompt_toolkit import PromptSession
from prompt_toolkit.history import FileHistory
from rich.console import Console

from agentic_crawler.agent.loop import run_agent
from agentic_crawler.config import Settings
from agentic_crawler.tui.display import ReplDisplay

HISTORY_FILE = Path.home() / ".agentic_crawler" / "history"
PROMPT = "\U0001f577\ufe0f > "


class ReplLoop:
    def __init__(self, settings: Settings, display: ReplDisplay) -> None:
        self.settings = settings
        self.display = display
        self._history_file = HISTORY_FILE

    def _print_banner(self) -> None:
        console: Console = self.display.get_console()
        console.print("\n[bold cyan]AgenticCrawler[/bold cyan] - Interactive REPL")
        console.print(
            f"Provider: {self.settings.llm_provider}  |  "
            "Type a goal to start crawling, Ctrl+D to exit\n"
        )

    async def run(self) -> None:
        self._history_file.parent.mkdir(parents=True, exist_ok=True)
        session: PromptSession[str] = PromptSession(history=FileHistory(str(self._history_file)))
        self._print_banner()

        while True:
            try:
                user_input = await session.prompt_async(PROMPT)
            except EOFError:
                self.display.get_console().print("\n[dim]Goodbye![/dim]")
                break
            except KeyboardInterrupt:
                self.display.get_console().print("\n[dim]Interrupted. Type Ctrl+D to exit.[/dim]")
                continue

            user_input = user_input.strip()
            if not user_input:
                continue

            try:
                await run_agent(
                    goal=user_input,
                    settings=self.settings,
                    display=self.display,
                )
            except KeyboardInterrupt:
                self.display.get_console().print("\n[dim]Agent interrupted.[/dim]")
            except Exception as exc:
                self.display.get_console().print(f"\n[bold red]Error:[/bold red] {exc}")
