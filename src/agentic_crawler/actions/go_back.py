from __future__ import annotations

from typing import Any

from agentic_crawler.actions.base import ActionResult
from agentic_crawler.fetcher.router import FetcherRouter


class GoBackAction:
    name = "go_back"
    description = "Navigate back to the previous page in browser history"

    async def execute(self, router: FetcherRouter, params: dict[str, Any]) -> ActionResult:
        await router.ensure_browser_ready()
        result = await router.browser.go_back()
        return ActionResult(
            success=result.success,
            observation=result.observation,
            new_url=result.new_url,
            new_html=result.new_html,
        )
