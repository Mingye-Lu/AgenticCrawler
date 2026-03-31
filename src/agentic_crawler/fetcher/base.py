from __future__ import annotations

from dataclasses import dataclass, field
from typing import Protocol, runtime_checkable


@dataclass
class FetchResult:
    url: str
    status_code: int
    html: str
    headers: dict[str, str] = field(default_factory=dict)


@runtime_checkable
class Fetcher(Protocol):
    async def get(self, url: str) -> FetchResult: ...
    async def close(self) -> None: ...


@dataclass
class BrowserAction:
    """Result of an interactive browser action."""

    success: bool
    observation: str
    screenshot: bytes | None = None
    new_url: str | None = None
    new_html: str | None = None
