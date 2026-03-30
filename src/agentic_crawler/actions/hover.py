from __future__ import annotations

from typing import Any

from agentic_crawler.actions.base import ActionResult
from agentic_crawler.fetcher.router import FetcherRouter


class HoverAction:
    name = "hover"
    description = "Hover over an element to reveal hidden content like dropdown menus or tooltips"

    async def execute(self, router: FetcherRouter, params: dict[str, Any]) -> ActionResult:
        selector = params.get("selector", "")
        if not selector:
            return ActionResult(success=False, observation="No selector provided")

        await router.ensure_browser_ready()
        result = await router.browser.hover(selector)
        return ActionResult(
            success=result.success,
            observation=result.observation,
            new_html=result.new_html,
        )
