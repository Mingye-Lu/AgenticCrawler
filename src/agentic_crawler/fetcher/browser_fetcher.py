from __future__ import annotations

from playwright.async_api import Browser, Page, async_playwright, Playwright

from agentic_crawler.fetcher.base import BrowserAction, FetchResult


class BrowserFetcher:
    def __init__(self, headless: bool = True, timeout: int = 30000) -> None:
        self.headless = headless
        self.timeout = timeout
        self._playwright: Playwright | None = None
        self._browser: Browser | None = None
        self._page: Page | None = None

    async def _ensure_browser(self) -> Page:
        if self._page is None:
            self._playwright = await async_playwright().start()
            self._browser = await self._playwright.chromium.launch(headless=self.headless)
            self._page = await self._browser.new_page()
            self._page.set_default_timeout(self.timeout)
        return self._page

    @property
    def page(self) -> Page | None:
        return self._page

    async def get(self, url: str) -> FetchResult:
        page = await self._ensure_browser()
        response = await page.goto(url, wait_until="domcontentloaded")
        html = await page.content()
        return FetchResult(
            url=page.url,
            status_code=response.status if response else 0,
            html=html,
        )

    async def click(self, selector: str) -> BrowserAction:
        page = await self._ensure_browser()
        try:
            await page.click(selector, timeout=self.timeout)
            await page.wait_for_load_state("domcontentloaded")
            return BrowserAction(
                success=True,
                observation=f"Clicked '{selector}'. Now on {page.url}",
                new_url=page.url,
                new_html=await page.content(),
            )
        except Exception as e:
            return BrowserAction(success=False, observation=f"Click failed on '{selector}': {e}")

    async def fill(self, selector: str, value: str) -> BrowserAction:
        page = await self._ensure_browser()
        try:
            await page.fill(selector, value)
            return BrowserAction(
                success=True,
                observation=f"Filled '{selector}' with value",
            )
        except Exception as e:
            return BrowserAction(success=False, observation=f"Fill failed on '{selector}': {e}")

    async def submit_form(self, selector: str = "form") -> BrowserAction:
        page = await self._ensure_browser()
        try:
            await page.evaluate(f'document.querySelector("{selector}").submit()')
            await page.wait_for_load_state("domcontentloaded")
            return BrowserAction(
                success=True,
                observation=f"Submitted form '{selector}'. Now on {page.url}",
                new_url=page.url,
                new_html=await page.content(),
            )
        except Exception as e:
            return BrowserAction(success=False, observation=f"Form submit failed: {e}")

    async def scroll(self, direction: str = "down", amount: int = 500) -> BrowserAction:
        page = await self._ensure_browser()
        delta = amount if direction == "down" else -amount
        await page.mouse.wheel(0, delta)
        await page.wait_for_timeout(500)
        return BrowserAction(
            success=True,
            observation=f"Scrolled {direction} by {amount}px",
            new_html=await page.content(),
        )

    async def screenshot(self) -> BrowserAction:
        page = await self._ensure_browser()
        data = await page.screenshot(type="png")
        return BrowserAction(
            success=True,
            observation=f"Screenshot taken of {page.url}",
            screenshot=data,
        )

    async def wait_for(self, selector: str | None = None, timeout: float = 5000) -> BrowserAction:
        page = await self._ensure_browser()
        try:
            if selector:
                await page.wait_for_selector(selector, timeout=timeout)
                return BrowserAction(success=True, observation=f"Element '{selector}' appeared")
            else:
                await page.wait_for_timeout(timeout)
                return BrowserAction(success=True, observation=f"Waited {timeout}ms")
        except Exception as e:
            return BrowserAction(success=False, observation=f"Wait failed: {e}")

    async def get_current_html(self) -> str:
        page = await self._ensure_browser()
        return await page.content()

    async def get_current_url(self) -> str:
        page = await self._ensure_browser()
        return page.url

    async def close(self) -> None:
        if self._page:
            await self._page.close()
        if self._browser:
            await self._browser.close()
        if self._playwright:
            await self._playwright.stop()
        self._page = None
        self._browser = None
        self._playwright = None
