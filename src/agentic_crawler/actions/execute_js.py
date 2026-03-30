from __future__ import annotations

from typing import Any

from agentic_crawler.actions.base import ActionResult
from agentic_crawler.fetcher.router import FetcherRouter


class ExecuteJsAction:
    name = "execute_js"
    description = "Execute JavaScript in the page context and return the result"

    async def execute(self, router: FetcherRouter, params: dict[str, Any]) -> ActionResult:
        script = params.get("script", "")
        if not script:
            return ActionResult(success=False, observation="No script provided")

        await router.ensure_browser_ready()
        result = await router.browser.execute_js(script)
        return ActionResult(
            success=result.success,
            observation=result.observation,
            new_html=result.new_html,
        )
