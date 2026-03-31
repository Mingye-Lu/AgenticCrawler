from __future__ import annotations

import asyncio
import re
import uuid
from datetime import datetime, timezone
from typing import Any

import structlog
from rich.console import Console
from rich.markdown import Markdown
from rich.panel import Panel

from agentic_crawler.agent.manager import AgentManager
from agentic_crawler.agent.prompt_builder import build_messages, build_plan_messages
from agentic_crawler.agent.state import AgentState, StepRecord
from agentic_crawler.agent.tools import get_action_registry, get_tool_schemas
from agentic_crawler.config import Settings
from agentic_crawler.fetcher.browser_fetcher import BrowserFetcher
from agentic_crawler.fetcher.router import FetcherRouter
from agentic_crawler.llm.base import LLMProvider
from agentic_crawler.output.writer import format_text, write_output
from agentic_crawler.parser.html_parser import page_content_to_text, parse_html

logger = structlog.get_logger()

MAX_CONSECUTIVE_ERRORS = 5
MAX_TEXT_ONLY_RESPONSES = 3
MAX_TEXT_ONLY_RETRIES = 3


class CrawlAgent:
    def __init__(
        self,
        agent_id: str,
        state: AgentState,
        settings: Settings,
        provider: LLMProvider,
        manager: AgentManager,
        router: FetcherRouter | None = None,
        owns_router: bool | None = None,
        is_root: bool = True,
        console: Console | None = None,
    ) -> None:
        self.agent_id = agent_id
        self.state = state
        self.settings = settings
        self.provider = provider
        self.manager = manager
        self.router = router
        self.is_root = is_root
        self.console = console or Console()
        self.actions = get_action_registry()
        self.tool_schemas = get_tool_schemas()
        self._child_agents: dict[str, CrawlAgent] = {}
        self._text_only_count = 0
        self._text_only_retries = 0
        self._thinking_started = False
        self._owns_router = owns_router if owns_router is not None else (router is None)

        # Compute output prefix based on agent type
        if is_root:
            self._output_prefix = "[bold dim][root][/bold dim]"
        else:
            short_id = agent_id[:6] if len(agent_id) >= 6 else agent_id
            self._output_prefix = f"[bold dim][{short_id}][/bold dim]"

    def _print(self, msg: str) -> None:
        """Print with agent ID prefix for multi-agent output clarity."""
        self.console.print(f"{self._output_prefix} {msg}")

    async def run(self, verbose: bool = False) -> None:
        if self.is_root:
            self.console.print(
                Panel("[bold]Planning...[/bold]", title=self._output_prefix, style="blue")
            )
            self.state.plan = await _plan(
                self.provider,
                self.state.goal,
                self.settings.temperature,
                self._on_thinking,
            )
            if self._thinking_started:
                self.console.file.write("\n")
                self.console.file.flush()
                self._thinking_started = False
            for i, step in enumerate(self.state.plan, 1):
                self._print(f"  {i}. {step}")
            self.manager.register_root(self.agent_id)

        if self.router is None:
            self.router = FetcherRouter(
                headless=self.settings.headless,
                browser_timeout=self.settings.browser_timeout,
            )

        self.console.print(
            Panel("[bold]Executing...[/bold]", title=self._output_prefix, style="green")
        )

        try:
            while not self.state.done and self.state.step_count < self.state.max_steps:
                if self.state.consecutive_errors >= MAX_CONSECUTIVE_ERRORS:
                    self.state.mark_done(
                        f"Stopped after {MAX_CONSECUTIVE_ERRORS} consecutive errors"
                    )
                    break

                if self._text_only_count >= MAX_TEXT_ONLY_RESPONSES:
                    if self._text_only_retries < MAX_TEXT_ONLY_RETRIES:
                        self._text_only_retries += 1
                        self._text_only_count = 0
                        self._print(
                            f"  [yellow]Agent returned text without tool calls. "
                            f"Retrying ({self._text_only_retries}/{MAX_TEXT_ONLY_RETRIES})...[/yellow]"
                        )
                        continue
                    self.state.mark_done(
                        "Agent produced text responses without tool calls. "
                        "Finishing with data collected so far."
                    )
                    break

                self._thinking_started = False
                active_children_info = [
                    {"id": child_id, "sub_goal": child_agent.state.goal}
                    for child_id, child_agent in self._child_agents.items()
                    if self.manager._agents.get(child_id)
                    and self.manager._agents[child_id].status == "active"
                ]
                messages = build_messages(
                    self.state,
                    provider=self.settings.llm_provider,
                    active_children=active_children_info if active_children_info else None,
                )
                response = await self.provider.complete(
                    messages=messages,
                    tools=self.tool_schemas,
                    temperature=self.settings.temperature,
                    on_thinking=self._on_thinking,
                )
                if self._thinking_started:
                    self.console.file.write("\n")
                    self.console.file.flush()
                self.state.total_tokens += sum(response.usage.values())

                if response.has_tool_calls:
                    self._text_only_count = 0
                    for tool_call in response.tool_calls:
                        await self._execute_tool_call(
                            name=tool_call.name,
                            params=tool_call.arguments,
                            tool_call_id=tool_call.id,
                            verbose=verbose,
                        )
                        if self.state.done:
                            break
                elif response.text:
                    if _text_signals_done(response.text):
                        self.state.mark_done(response.text)
                    else:
                        self._text_only_count += 1
                        self.state.add_text_response(response.text)
                        if verbose:
                            self._print(f"[dim]Agent: {response.text[:200]}[/dim]")

                if self.state.current_html and self.state.current_url:
                    content = parse_html(self.state.current_html, self.state.current_url)
                    self.state.page_summary = page_content_to_text(content)

            if self.state.extracted_data:
                self.console.print(
                    Panel(
                        f"[bold green]Done![/bold green] Extracted {len(self.state.extracted_data)} item(s)",
                        title=self._output_prefix,
                    )
                )
                self.console.print(
                    Markdown(format_text(self.state.extracted_data, summary=self.state.done_reason))
                )
                if self.settings.output_file:
                    write_output(
                        self.state.extracted_data,
                        self.settings.output_format,
                        self.settings.output_file,
                    )
            else:
                self.console.print(
                    Panel(
                        f"[bold yellow]Done.[/bold yellow] "
                        f"{self.state.done_reason or 'No data extracted.'}",
                        title=self._output_prefix,
                    )
                )

            self._print(
                f"Steps: {self.state.step_count} | Tokens: {self.state.total_tokens} | Errors: {len(self.state.errors)}"
            )
        finally:
            self.manager.mark_done(self.agent_id)
            if self.router is not None and self._owns_router:
                await self.router.close()

    async def fork(self, sub_goal: str, url: str | None = None) -> str:
        if not self.manager.can_fork(self.agent_id):
            return "Cannot fork: fork limits exceeded (max_concurrent_per_parent, max_depth, or max_total)"

        child_id = f"fork-{uuid.uuid4().hex[:8]}"
        target_url = url or self.state.current_url
        child_state = self.state.fork(sub_goal=sub_goal, url=target_url)
        child_state.max_steps = self.settings.fork_child_max_steps

        child_router: FetcherRouter | None = None
        parent_context = None
        if self.router is not None and hasattr(self.router, "browser"):
            parent_context = getattr(self.router.browser, "_context", None)

        if parent_context is not None:
            child_browser = BrowserFetcher(
                headless=self.settings.headless,
                timeout=self.settings.browser_timeout,
                context=parent_context,
            )
            await child_browser._ensure_browser()
            if target_url and child_browser.page is not None:
                try:
                    await child_browser.page.goto(target_url, wait_until="domcontentloaded")
                except Exception:
                    pass

            child_router = FetcherRouter(
                headless=self.settings.headless,
                browser_timeout=self.settings.browser_timeout,
                browser_fetcher=child_browser,
            )
            child_router.escalate_to_browser()

        child_agent = CrawlAgent(
            agent_id=child_id,
            state=child_state,
            settings=self.settings,
            provider=self.provider,
            manager=self.manager,
            router=child_router,
            owns_router=True,
            is_root=False,
            console=self.console,
        )

        task = asyncio.create_task(child_agent.run())
        self.manager.register_child(child_id, self.agent_id, task)
        self._child_agents[child_id] = child_agent

        return child_id

    async def _wait_for_children(self) -> str:
        """Wait for all active child agents and merge their data. Returns summary observation."""
        child_tasks = self.manager.get_child_tasks(self.agent_id)
        if not child_tasks:
            return "No active subagents"

        try:
            await asyncio.wait_for(
                asyncio.gather(*child_tasks, return_exceptions=True),
                timeout=self.settings.fork_wait_timeout,
            )
        except asyncio.TimeoutError:
            self._print(
                "[yellow]Warning: fork_wait_timeout exceeded, collecting partial results[/yellow]"
            )

        total_items = 0
        for child_id, child_agent in list(self._child_agents.items()):
            if self.manager._agents.get(child_id, None) is not None:
                total_items += self._merge_child_results(child_id, child_agent)

        return f"Waited for {len(child_tasks)} subagent(s). Collected {total_items} total item(s)."

    def _merge_child_results(self, child_id: str, child_agent: CrawlAgent) -> int:
        """Merge child's extracted_data into parent. Returns number of items merged."""
        items = child_agent.state.extracted_data
        self.state.extracted_data.extend(items)
        n = len(items)
        if n > 0:
            sub_goal = child_agent.state.goal
            obs = f"Subagent {child_id} completed: extracted {n} item(s) for '{sub_goal}'"
            self.state.history.append(
                StepRecord(
                    action="__child_merge__",
                    params={"child_id": child_id},
                    observation=obs,
                    success=True,
                )
            )
        return n

    def _on_thinking(self, chunk: str) -> None:
        if not self._thinking_started:
            self._print("[dim italic]  Thinking...[/dim italic]")
            self._thinking_started = True
        self.console.file.write(chunk)
        self.console.file.flush()

    async def _execute_tool_call(
        self,
        name: str,
        params: dict[str, Any],
        tool_call_id: str,
        verbose: bool,
    ) -> None:
        ts = datetime.now(timezone.utc).strftime("%H:%M:%S")
        step_label = f"[Step {self.state.step_count + 1}]"

        if name == "done":
            summary = params.get("summary", "Task completed")
            child_tasks = self.manager.get_child_tasks(self.agent_id)
            if child_tasks:
                self._print(
                    f"  {step_label} [dim]{ts}[/dim] [dim]Waiting for {len(child_tasks)} child agent(s)...[/dim]"
                )
                await self._wait_for_children()
            self.state.mark_done(summary)
            self._print(f"  {step_label} [dim]{ts}[/dim] [bold green]Done:[/bold green] {summary}")
            return

        if name == "fork":
            sub_goal = params.get("sub_goal", "")
            url = params.get("url")
            child_id = await self.fork(sub_goal, url)
            if child_id.startswith("Cannot fork"):
                observation = child_id
                success = False
            else:
                observation = f"Forked subagent {child_id} for: {sub_goal}"
                success = True
            self.state.add_step(
                name, params, observation, success=success, tool_call_id=tool_call_id
            )
            self._print(f"  {step_label} [dim]{ts}[/dim] [cyan]{observation}[/cyan]")
            return

        if name == "wait_for_subagents":
            observation = await self._wait_for_children()
            self.state.add_step(name, params, observation, success=True, tool_call_id=tool_call_id)
            self._print(f"  {step_label} [dim]{ts}[/dim] [cyan]{observation}[/cyan]")
            return

        action = self.actions.get(name)
        if not action:
            self.state.add_step(name, params, f"Unknown action: {name}", success=False)
            self._print(f"  {step_label} [dim]{ts}[/dim] [red]Unknown action: {name}[/red]")
            return

        self._print(f"  {step_label} [dim]{ts}[/dim] {name}({_compact_params(params)})")
        if self.router is None:
            raise RuntimeError("Router is not initialized")
        result = await action.execute(self.router, params)

        self.state.add_step(
            name, params, result.observation, result.success, tool_call_id=tool_call_id
        )

        if result.new_url:
            self.state.current_url = result.new_url
        if result.new_html:
            self.state.current_html = result.new_html
        if result.data is not None:
            self.state.extracted_data.append(result.data)

        status = "[green]OK[/green]" if result.success else "[red]FAIL[/red]"
        if verbose:
            self._print(f"    {status} {result.observation}")
        else:
            self._print(f"    {status}")


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

    lines = response.text.strip().splitlines()
    plan: list[str] = []
    for line in lines:
        line = line.strip()
        cleaned = re.sub(r"^(\d+[\.\)]\s*|-\s*)", "", line).strip()
        if cleaned:
            plan.append(cleaned)
    return plan or ["Navigate to the target website", "Extract the requested data", "Done"]


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
