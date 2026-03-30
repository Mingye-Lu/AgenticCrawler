from __future__ import annotations

import httpx

from agentic_crawler.fetcher.base import FetchResult


class HttpFetcher:
    def __init__(self, timeout: float = 30.0) -> None:
        self.client = httpx.AsyncClient(
            timeout=timeout,
            follow_redirects=True,
            headers={
                "User-Agent": (
                    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) "
                    "AppleWebKit/537.36 (KHTML, like Gecko) "
                    "Chrome/131.0.0.0 Safari/537.36"
                )
            },
        )

    async def get(self, url: str) -> FetchResult:
        response = await self.client.get(url)
        return FetchResult(
            url=str(response.url),
            status_code=response.status_code,
            html=response.text,
            headers=dict(response.headers),
        )

    async def post(self, url: str, data: dict | None = None, json: dict | None = None) -> FetchResult:
        response = await self.client.post(url, data=data, json=json)
        return FetchResult(
            url=str(response.url),
            status_code=response.status_code,
            html=response.text,
            headers=dict(response.headers),
        )

    async def close(self) -> None:
        await self.client.aclose()
