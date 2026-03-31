from __future__ import annotations

import asyncio
from typing import Optional

import typer
from rich.console import Console

from agentic_crawler.config import get_settings

app = typer.Typer(name="agentic-crawler", help="Autonomous LLM-powered web crawler")
console = Console()


@app.command()
def login() -> None:
    """Authenticate with OpenAI via OAuth for Codex models."""
    from agentic_crawler.llm.oauth import run_login_flow

    try:
        tokens = run_login_flow()
        console.print("[bold green]Authenticated successfully![/]")
        console.print(
            f"[dim]Token expires at {__import__('datetime').datetime.fromtimestamp(tokens.expires_at)}[/]"
        )
    except Exception as exc:
        console.print(f"[bold red]Login failed:[/] {exc}")
        raise typer.Exit(1) from exc


@app.command()
def run(
    goal: str = typer.Argument(help="What you want the crawler to do, in natural language"),
    provider: Optional[str] = typer.Option(None, "--provider", "-p", help="LLM provider"),
    model: Optional[str] = typer.Option(None, "--model", "-m", help="Model name"),
    max_steps: Optional[int] = typer.Option(None, "--max-steps", help="Max agent steps"),
    output: Optional[str] = typer.Option(None, "--output", "-o", help="Output file path"),
    output_format: Optional[str] = typer.Option(None, "--format", "-f", help="json, csv, stdout"),
    workspace: Optional[str] = typer.Option(None, "--workspace", "-w", help="Workspace directory for saved files"),
    headless: bool = typer.Option(True, "--headless/--no-headless", help="Browser headless mode"),
    verbose: bool = typer.Option(False, "--verbose", "-v", help="Verbose logging"),
) -> None:
    """Run the agentic crawler with a natural language goal."""
    overrides: dict[str, object] = {"headless": headless}
    if provider:
        overrides["llm_provider"] = provider
    if model:
        if provider == "claude":
            overrides["claude_model"] = model
        elif provider == "codex":
            overrides["codex_model"] = model
        else:
            overrides["openai_model"] = model
    if max_steps:
        overrides["max_steps"] = max_steps
    if output:
        overrides["output_file"] = output
    if output_format:
        overrides["output_format"] = output_format
    if workspace:
        overrides["workspace_dir"] = workspace

    settings = get_settings(**overrides)

    from agentic_crawler.utils.logging import setup_logging

    setup_logging(verbose=verbose)

    console.print(f"[bold green]Goal:[/] {goal}")
    console.print(f"[bold blue]Provider:[/] {settings.llm_provider}")
    console.print(f"[bold blue]Max steps:[/] {settings.max_steps}")

    from agentic_crawler.agent.loop import run_agent

    asyncio.run(run_agent(goal=goal, settings=settings, verbose=verbose))


if __name__ == "__main__":
    app()
