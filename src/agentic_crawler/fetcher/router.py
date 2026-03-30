from __future__ import annotations

from agentic_crawler.fetcher.browser_fetcher import BrowserFetcher
from agentic_crawler.fetcher.http_fetcher import HttpFetcher
from agentic_crawler.fetcher.base import FetchResult

# Actions that require a browser
BROWSER_ACTIONS = {"click", "fill_form", "scroll", "screenshot", "wait"}


class FetcherRouter:
    """Routes fetching to HTTP or browser based on the action needed."""

    def __init__(self, headless: bool = True, browser_timeout: int = 30000) -> None:
        self.http = HttpFetcher()
        self.browser = BrowserFetcher(headless=headless, timeout=browser_timeout)
        self._using_browser = False

    @property
    def needs_browser(self) -> bool:
        return self._using_browser

    def escalate_to_browser(self) -> None:
        """Switch to browser mode for this session."""
        self._using_browser = True

    async def get(self, url: str) -> FetchResult:
        if self._using_browser:
            return await self.browser.get(url)
        # Try HTTP first
        result = await self.http.get(url)
        # Auto-escalate if page looks JS-rendered (very little text content)
        if len(result.html.strip()) < 500 and "<noscript" in result.html.lower():
            self._using_browser = True
            return await self.browser.get(url)
        return result

    def should_use_browser(self, action_name: str) -> bool:
        if action_name in BROWSER_ACTIONS:
            self._using_browser = True
            return True
        return self._using_browser

    async def close(self) -> None:
        await self.http.close()
        await self.browser.close()
