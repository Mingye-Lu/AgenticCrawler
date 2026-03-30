from __future__ import annotations

from typing import Any

from agentic_crawler.actions.base import ActionResult
from agentic_crawler.fetcher.router import FetcherRouter


class FillFormAction:
    name = "fill_form"
    description = "Fill form fields and optionally submit"

    async def execute(self, router: FetcherRouter, params: dict[str, Any]) -> ActionResult:
        fields: dict[str, str] = params.get("fields", {})
        submit = params.get("submit", False)
        form_selector = params.get("form_selector", "form")

        if not fields:
            return ActionResult(success=False, observation="No fields provided")

        await router.ensure_browser_ready()

        observations: list[str] = []
        for selector, value in fields.items():
            result = await router.browser.fill(selector, value)
            observations.append(result.observation)
            if not result.success:
                return ActionResult(
                    success=False,
                    observation=f"Fill failed: {' | '.join(observations)}",
                )

        if submit:
            submit_result = await router.browser.submit_form(form_selector)
            observations.append(submit_result.observation)
            return ActionResult(
                success=submit_result.success,
                observation=" | ".join(observations),
                new_url=submit_result.new_url,
                new_html=submit_result.new_html,
            )

        return ActionResult(success=True, observation=" | ".join(observations))
