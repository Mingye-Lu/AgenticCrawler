from __future__ import annotations

from agentic_crawler.fetcher.browser_fetcher import BrowserFetcher
from agentic_crawler.fetcher.http_fetcher import HttpFetcher
from agentic_crawler.fetcher.base import FetchResult

BROWSER_ACTIONS = {
    "click",
    "fill_form",
    "scroll",
    "screenshot",
    "wait",
    "select_option",
    "go_back",
    "execute_js",
    "hover",
    "press_key",
    "switch_tab",
}


class FetcherRouter:
    def __init__(
        self,
        headless: bool = True,
        browser_timeout: int = 30000,
        browser_fetcher: BrowserFetcher | None = None,
    ) -> None:
        self.http = HttpFetcher()
        self.browser = browser_fetcher or BrowserFetcher(headless=headless, timeout=browser_timeout)
        # When headless=False the user explicitly wants a visible browser for all actions
        self._using_browser = not headless
        self._last_url: str | None = None

    @property
    def needs_browser(self) -> bool:
        return self._using_browser

    def escalate_to_browser(self) -> None:
        self._using_browser = True

    async def ensure_browser_ready(self) -> None:
        self._using_browser = True
        page = await self.browser._ensure_browser()
        if self._last_url and page.url in ("about:blank", ""):
            await page.goto(self._last_url, wait_until="domcontentloaded")

    async def get(self, url: str) -> FetchResult:
        if self._using_browser:
            result = await self.browser.get(url)
            self._last_url = result.url
            return result
        result = await self.http.get(url)
        self._last_url = result.url
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
