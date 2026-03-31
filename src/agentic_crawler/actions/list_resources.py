from __future__ import annotations

import re
from typing import Any

from agentic_crawler.actions.base import ActionResult
from agentic_crawler.fetcher.router import FetcherRouter


class ListResourcesAction:
    name = "list_resources"
    description = "List sub-resources loaded by the current page with optional regex filters"

    async def execute(self, router: FetcherRouter, params: dict[str, Any]) -> ActionResult:
        type_pattern = params.get("type_pattern", "")
        name_pattern = params.get("name_pattern", "")

        try:
            type_re = re.compile(type_pattern, re.IGNORECASE) if type_pattern else None
        except re.error as e:
            return ActionResult(success=False, observation=f"Invalid type_pattern regex: {e}")

        try:
            name_re = re.compile(name_pattern, re.IGNORECASE) if name_pattern else None
        except re.error as e:
            return ActionResult(success=False, observation=f"Invalid name_pattern regex: {e}")

        resources = router.browser.collected_resources

        matched = [
            r
            for r in resources
            if (type_re is None or type_re.search(r.resource_type))
            and (name_re is None or name_re.search(r.url))
        ]

        entries = [
            {
                "url": r.url,
                "type": r.resource_type,
                "status": r.status,
                "mime": r.mime_type,
                "size": r.size,
            }
            for r in matched
        ]

        observation = f"Found {len(matched)} resource(s) (of {len(resources)} total)"
        if type_pattern:
            observation += f" matching type=/{type_pattern}/"
        if name_pattern:
            observation += f" matching name=/{name_pattern}/"

        return ActionResult(success=True, observation=observation, data=entries)
