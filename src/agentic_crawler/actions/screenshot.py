from __future__ import annotations

import base64
from typing import Any

from agentic_crawler.actions.base import ActionResult
from agentic_crawler.fetcher.router import FetcherRouter


class ScreenshotAction:
    name = "screenshot"
    description = "Take a screenshot of the current page"

    async def execute(self, router: FetcherRouter, params: dict[str, Any]) -> ActionResult:
        await router.ensure_browser_ready()
        result = await router.browser.screenshot()

        screenshot_b64 = ""
        if result.screenshot:
            screenshot_b64 = base64.b64encode(result.screenshot).decode()

        return ActionResult(
            success=result.success,
            observation=result.observation,
            data={"screenshot_base64": screenshot_b64} if screenshot_b64 else None,
        )
