from __future__ import annotations

import uuid

from rich.console import Console

from agentic_crawler.agent.display import AgentDisplay, LiveDashboard
from agentic_crawler.agent.manager import AgentManager
from agentic_crawler.agent.state import AgentState
from agentic_crawler.config import Settings
from agentic_crawler.fetcher.router import FetcherRouter  # noqa: F401
from agentic_crawler.llm.registry import get_provider

console = Console()


async def run_agent(
    goal: str,
    settings: Settings,
    verbose: bool = False,
    display: AgentDisplay | None = None,
) -> None:
    from agentic_crawler.agent.crawl_agent import CrawlAgent

    provider = get_provider(settings)
    manager = AgentManager(
        max_concurrent_per_parent=settings.max_concurrent_per_parent,
        max_depth=settings.max_fork_depth,
        max_total=settings.max_total_agents,
    )
    state = AgentState(goal=goal, max_steps=settings.max_steps)
    agent_id = str(uuid.uuid4())

    if display is None:
        display = LiveDashboard(console=console, verbose=verbose)
        display.register_agent(agent_id, goal, None, settings.max_steps)
        display.start()
    else:
        display.register_agent(agent_id, goal, None, settings.max_steps)
    agent = CrawlAgent(
        agent_id=agent_id,
        state=state,
        settings=settings,
        provider=provider,
        manager=manager,
        is_root=True,
        display=display,
    )
    try:
        await agent.run(verbose=verbose)
    finally:
        await provider.close()
