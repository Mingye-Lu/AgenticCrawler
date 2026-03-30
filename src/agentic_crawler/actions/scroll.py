from __future__ import annotations

from typing import Any

from agentic_crawler.actions.base import ActionResult
from agentic_crawler.fetcher.router import FetcherRouter


class ScrollAction:
    name = "scroll"
    description = "Scroll the page up or down"

    async def execute(self, router: FetcherRouter, params: dict[str, Any]) -> ActionResult:
        direction = params.get("direction", "down")
        amount = params.get("amount", 500)

        await router.ensure_browser_ready()
        result = await router.browser.scroll(direction=direction, amount=amount)
        return ActionResult(
            success=result.success,
            observation=result.observation,
            new_html=result.new_html,
        )
