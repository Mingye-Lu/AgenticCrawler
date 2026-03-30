from __future__ import annotations

from typing import Any

from agentic_crawler.actions.base import ActionResult
from agentic_crawler.fetcher.router import FetcherRouter


class WaitAction:
    name = "wait"
    description = "Wait for a selector to appear or for a fixed duration"

    async def execute(self, router: FetcherRouter, params: dict[str, Any]) -> ActionResult:
        selector = params.get("selector")
        seconds = params.get("seconds", 2.0)

        await router.ensure_browser_ready()
        timeout_ms = seconds * 1000 if not selector else 10000
        result = await router.browser.wait_for(selector=selector, timeout=timeout_ms)
        return ActionResult(success=result.success, observation=result.observation)
