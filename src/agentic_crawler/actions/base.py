from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Protocol, runtime_checkable

from agentic_crawler.fetcher.router import FetcherRouter


@dataclass
class ActionResult:
    success: bool
    observation: str
    data: Any | None = None
    new_url: str | None = None
    new_html: str | None = None


@runtime_checkable
class Action(Protocol):
    name: str
    description: str

    async def execute(self, router: FetcherRouter, params: dict[str, Any]) -> ActionResult: ...
