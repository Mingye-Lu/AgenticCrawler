from __future__ import annotations

from typing import Any

from agentic_crawler.actions.base import ActionResult
from agentic_crawler.fetcher.router import FetcherRouter


class NavigateAction:
    name = "navigate"
    description = "Navigate to a URL"

    async def execute(self, router: FetcherRouter, params: dict[str, Any]) -> ActionResult:
        url = params.get("url", "")
        if not url:
            return ActionResult(success=False, observation="No URL provided")

        try:
            result = await router.get(url)
            return ActionResult(
                success=True,
                observation=f"Navigated to {result.url} (status {result.status_code})",
                new_url=result.url,
                new_html=result.html,
            )
        except Exception as e:
            return ActionResult(success=False, observation=f"Navigation failed: {e}")
