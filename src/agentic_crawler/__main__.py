from __future__ import annotations

import argparse
import asyncio
import sys


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="agentic-crawler",
        description="Autonomous LLM-powered web crawler — interactive REPL",
    )
    parser.add_argument("--provider", "-p", help="LLM provider (claude, openai, codex)")
    parser.add_argument("--model", "-m", help="Model name override")
    parser.add_argument("--max-steps", type=int, help="Max agent steps")
    parser.add_argument("--workspace", "-w", help="Workspace directory")
    parser.add_argument("--no-headless", action="store_true", help="Show browser window")
    parser.add_argument("--verbose", "-v", action="store_true", help="Verbose logging")

    subparsers = parser.add_subparsers(dest="command")
    subparsers.add_parser(
        "login",
        help="Authenticate with OpenAI via OAuth for Codex models",
    )

    return parser


def _run_login() -> None:
    from rich.console import Console
    from agentic_crawler.llm.oauth import run_login_flow

    console = Console()
    try:
        tokens = run_login_flow()
        import datetime

        console.print("[bold green]Authenticated successfully![/]")
        console.print(
            f"[dim]Token expires at {datetime.datetime.fromtimestamp(tokens.expires_at)}[/]"
        )
    except Exception as exc:
        console.print(f"[bold red]Login failed:[/bold red] {exc}")
        sys.exit(1)


def _run_repl(args: argparse.Namespace) -> None:
    from rich.console import Console
    from agentic_crawler.config import get_settings
    from agentic_crawler.tui.display import ReplDisplay
    from agentic_crawler.tui.repl import ReplLoop
    from agentic_crawler.utils.logging import setup_logging

    overrides: dict[str, object] = {}
    if args.provider:
        overrides["llm_provider"] = args.provider
    if args.model:
        provider = args.provider or "claude"
        if provider == "claude":
            overrides["claude_model"] = args.model
        elif provider == "codex":
            overrides["codex_model"] = args.model
        else:
            overrides["openai_model"] = args.model
    if args.max_steps:
        overrides["max_steps"] = args.max_steps
    if args.workspace:
        overrides["workspace_dir"] = args.workspace
    if args.no_headless:
        overrides["headless"] = False

    settings = get_settings(**overrides)
    setup_logging(verbose=args.verbose)

    console = Console()
    display = ReplDisplay(console=console, verbose=args.verbose)
    repl = ReplLoop(settings=settings, display=display)
    asyncio.run(repl.run())


def main(argv: list[str] | None = None) -> None:
    parser = _build_parser()
    args = parser.parse_args(argv)

    if args.command == "login":
        _run_login()
    else:
        _run_repl(args)


if __name__ == "__main__":
    main()
