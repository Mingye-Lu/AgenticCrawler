from __future__ import annotations

from typing import Any

from pydantic import BaseModel


class CrawlResult(BaseModel):
    goal: str
    steps_taken: int
    total_tokens: int
    data: list[Any]
    errors: list[str]
    done_reason: str
