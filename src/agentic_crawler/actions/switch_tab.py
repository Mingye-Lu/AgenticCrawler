from __future__ import annotations

from typing import Any

from agentic_crawler.actions.base import ActionResult
from agentic_crawler.fetcher.router import FetcherRouter


class SwitchTabAction:
    name = "switch_tab"
    description = (
        "Switch to a different browser tab. Use index -1 for the newest tab, 0 for the first."
    )

    async def execute(self, router: FetcherRouter, params: dict[str, Any]) -> ActionResult:
        index = params.get("index", -1)

        await router.ensure_browser_ready()
        result = await router.browser.switch_tab(index=index)
        return ActionResult(
            success=result.success,
            observation=result.observation,
            new_url=result.new_url,
            new_html=result.new_html,
        )
