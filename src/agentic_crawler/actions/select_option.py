from __future__ import annotations

from typing import Any

from agentic_crawler.actions.base import ActionResult
from agentic_crawler.fetcher.router import FetcherRouter


class SelectOptionAction:
    name = "select_option"
    description = "Select an option from a dropdown/select element"

    async def execute(self, router: FetcherRouter, params: dict[str, Any]) -> ActionResult:
        selector = params.get("selector", "")
        if not selector:
            return ActionResult(success=False, observation="No selector provided")

        value = params.get("value")
        label = params.get("label")
        index = params.get("index")

        if value is None and label is None and index is None:
            return ActionResult(
                success=False,
                observation="Provide 'value', 'label', or 'index' to select an option",
            )

        await router.ensure_browser_ready()
        result = await router.browser.select_option(selector, value=value, label=label, index=index)
        return ActionResult(
            success=result.success,
            observation=result.observation,
            new_html=result.new_html,
        )
