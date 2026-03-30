from __future__ import annotations

from typing import Any

from agentic_crawler.actions.base import ActionResult
from agentic_crawler.fetcher.router import FetcherRouter


class ExtractDataAction:
    name = "extract_data"
    description = "Extract structured data from the current page based on an instruction"

    async def execute(self, router: FetcherRouter, params: dict[str, Any]) -> ActionResult:
        instruction = params.get("instruction", "")
        data = params.get("data")

        if not data and not instruction:
            return ActionResult(
                success=False,
                observation="Provide 'data' (the extracted data) or 'instruction' for what to extract",
            )

        # The agent extracts data by providing it directly in the tool call
        # The data is whatever the LLM parsed from the page content in context
        return ActionResult(
            success=True,
            observation=f"Extracted data per instruction: {instruction}" if instruction else "Data extracted",
            data=data,
        )
