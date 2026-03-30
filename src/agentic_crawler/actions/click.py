from __future__ import annotations

from typing import Any

from agentic_crawler.actions.base import ActionResult
from agentic_crawler.fetcher.router import FetcherRouter


class ClickAction:
    name = "click"
    description = "Click an element on the page by CSS selector or text content"

    async def execute(self, router: FetcherRouter, params: dict[str, Any]) -> ActionResult:
        selector = params.get("selector", "")
        text = params.get("text", "")

        if not selector and not text:
            return ActionResult(success=False, observation="No selector or text provided")

        router.escalate_to_browser()

        # If text is provided, convert to a text-based selector
        if text and not selector:
            selector = f"text={text}"

        result = await router.browser.click(selector)
        return ActionResult(
            success=result.success,
            observation=result.observation,
            new_url=result.new_url,
            new_html=result.new_html,
        )
