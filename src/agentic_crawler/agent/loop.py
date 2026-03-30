from __future__ import annotations

import re
from datetime import datetime, timezone
from typing import Any

import structlog
from rich.console import Console
from rich.panel import Panel

from agentic_crawler.agent.prompt_builder import build_messages, build_plan_messages
from agentic_crawler.agent.state import AgentState
from agentic_crawler.agent.tools import get_action_registry, get_tool_schemas
from agentic_crawler.config import Settings
from agentic_crawler.fetcher.router import FetcherRouter
from agentic_crawler.llm.base import LLMProvider
from agentic_crawler.llm.registry import get_provider
from agentic_crawler.output.writer import write_output
from agentic_crawler.parser.html_parser import page_content_to_text, parse_html

logger = structlog.get_logger()
console = Console()

MAX_CONSECUTIVE_ERRORS = 5
MAX_TEXT_ONLY_RESPONSES = 3
MAX_TEXT_ONLY_RETRIES = 3


async def run_agent(goal: str, settings: Settings, verbose: bool = False) -> None:
    provider = get_provider(settings)
    router = FetcherRouter(headless=settings.headless, browser_timeout=settings.browser_timeout)
    actions = get_action_registry()
    tool_schemas = get_tool_schemas()

    state = AgentState(goal=goal, max_steps=settings.max_steps)
    text_only_count = 0
    text_only_retries = 0

    try:
        thinking_started = False

        def _on_thinking(chunk: str) -> None:
            nonlocal thinking_started
            if not thinking_started:
                console.print("[dim italic]  Thinking...[/dim italic]")
                thinking_started = True
            # Stream each chunk directly — no newline so tokens flow continuously
            console.file.write(chunk)
            console.file.flush()

        console.print(Panel("[bold]Planning...[/bold]", style="blue"))
        state.plan = await _plan(provider, goal, settings.temperature, _on_thinking)
        if thinking_started:
            console.file.write("\n")
            console.file.flush()
            thinking_started = False
        for i, step in enumerate(state.plan, 1):
            console.print(f"  {i}. {step}")

        console.print(Panel("[bold]Executing...[/bold]", style="green"))

        while not state.done and state.step_count < state.max_steps:
            if state.consecutive_errors >= MAX_CONSECUTIVE_ERRORS:
                state.mark_done(f"Stopped after {MAX_CONSECUTIVE_ERRORS} consecutive errors")
                break

            if text_only_count >= MAX_TEXT_ONLY_RESPONSES:
                if text_only_retries < MAX_TEXT_ONLY_RETRIES:
                    text_only_retries += 1
                    text_only_count = 0
                    console.print(
                        f"  [yellow]Agent returned text without tool calls. "
                        f"Retrying ({text_only_retries}/{MAX_TEXT_ONLY_RETRIES})...[/yellow]"
                    )
                    continue
                state.mark_done(
                    "Agent produced text responses without tool calls. "
                    "Finishing with data collected so far."
                )
                break

            thinking_started = False
            messages = build_messages(state, provider=settings.llm_provider)
            response = await provider.complete(
                messages=messages,
                tools=tool_schemas,
                temperature=settings.temperature,
                on_thinking=_on_thinking,
            )
            if thinking_started:
                # End the thinking stream with a newline
                console.file.write("\n")
                console.file.flush()
            state.total_tokens += sum(response.usage.values())

            if response.has_tool_calls:
                text_only_count = 0
                for tool_call in response.tool_calls:
                    await _execute_tool_call(
                        tool_call.name,
                        tool_call.arguments,
                        tool_call.id,
                        state,
                        router,
                        actions,
                        verbose,
                    )
                    if state.done:
                        break
            elif response.text:
                if _text_signals_done(response.text):
                    state.mark_done(response.text)
                else:
                    text_only_count += 1
                    if verbose:
                        console.print(f"[dim]Agent: {response.text[:200]}[/dim]")

            # Update page summary if we have new HTML
            if state.current_html and state.current_url:
                content = parse_html(state.current_html, state.current_url)
                state.page_summary = page_content_to_text(content)

        # Output results
        if state.extracted_data:
            console.print(
                Panel(
                    f"[bold green]Done![/bold green] Extracted {len(state.extracted_data)} item(s)"
                )
            )
            write_output(state.extracted_data, settings.output_format, settings.output_file)
        else:
            console.print(
                Panel(
                    f"[bold yellow]Done.[/bold yellow] {state.done_reason or 'No data extracted.'}"
                )
            )

        # Stats
        console.print(
            f"Steps: {state.step_count} | Tokens: {state.total_tokens} | Errors: {len(state.errors)}"
        )

    finally:
        await router.close()
        await provider.close()


async def _plan(
    provider: LLMProvider,
    goal: str,
    temperature: float,
    on_thinking: Any = None,
) -> list[str]:
    messages = build_plan_messages(goal)
    response = await provider.complete(
        messages=messages, temperature=temperature, on_thinking=on_thinking
    )

    if not response.text:
        return ["Navigate to the target website", "Extract the requested data", "Done"]

    # Parse numbered list
    lines = response.text.strip().splitlines()
    plan = []
    for line in lines:
        line = line.strip()
        # Remove numbering like "1.", "1)", "- "
        cleaned = re.sub(r"^(\d+[\.\)]\s*|-\s*)", "", line).strip()
        if cleaned:
            plan.append(cleaned)
    return plan or ["Navigate to the target website", "Extract the requested data", "Done"]


async def _execute_tool_call(
    name: str,
    params: dict[str, Any],
    tool_call_id: str,
    state: AgentState,
    router: FetcherRouter,
    actions: dict[str, Any],
    verbose: bool,
) -> None:
    ts = datetime.now(timezone.utc).strftime("%H:%M:%S")
    step_label = f"[Step {state.step_count + 1}]"

    # Handle 'done' specially
    if name == "done":
        summary = params.get("summary", "Task completed")
        state.mark_done(summary)
        console.print(f"  {step_label} [dim]{ts}[/dim] [bold green]Done:[/bold green] {summary}")
        return

    action = actions.get(name)
    if not action:
        state.add_step(name, params, f"Unknown action: {name}", success=False)
        console.print(f"  {step_label} [dim]{ts}[/dim] [red]Unknown action: {name}[/red]")
        return

    # Execute
    console.print(f"  {step_label} [dim]{ts}[/dim] {name}({_compact_params(params)})")
    result = await action.execute(router, params)

    # Update state
    state.add_step(name, params, result.observation, result.success, tool_call_id=tool_call_id)

    if result.new_url:
        state.current_url = result.new_url
    if result.new_html:
        state.current_html = result.new_html
    if result.data is not None:
        state.extracted_data.append(result.data)

    status = "[green]OK[/green]" if result.success else "[red]FAIL[/red]"
    if verbose:
        console.print(f"    {status} {result.observation}")
    else:
        console.print(f"    {status}")


def _text_signals_done(text: str) -> bool:
    lower = text.lower()
    return any(
        phrase in lower
        for phrase in ["task complete", "goal achieved", "i'm done", "task is done", "finished"]
    )


def _compact_params(params: dict[str, Any]) -> str:
    parts = []
    for k, v in params.items():
        s = str(v)
        if len(s) > 60:
            s = s[:57] + "..."
        parts.append(f"{k}={s}")
    return ", ".join(parts)
