from __future__ import annotations

from typing import Any

from agentic_crawler.actions.base import ActionResult
from agentic_crawler.fetcher.router import FetcherRouter


class PressKeyAction:
    name = "press_key"
    description = "Press a keyboard key (Enter, Escape, Tab, ArrowDown, etc.) optionally on a specific element"

    async def execute(self, router: FetcherRouter, params: dict[str, Any]) -> ActionResult:
        key = params.get("key", "")
        if not key:
            return ActionResult(success=False, observation="No key provided")

        selector = params.get("selector")

        await router.ensure_browser_ready()
        result = await router.browser.press_key(key, selector=selector)
        return ActionResult(
            success=result.success,
            observation=result.observation,
            new_url=result.new_url,
            new_html=result.new_html,
        )
