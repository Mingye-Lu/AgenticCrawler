from __future__ import annotations

from playwright.async_api import Browser, BrowserContext, Page, async_playwright, Playwright

from agentic_crawler.fetcher.base import BrowserAction, FetchResult

_USER_AGENT = (
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) "
    "AppleWebKit/537.36 (KHTML, like Gecko) "
    "Chrome/131.0.0.0 Safari/537.36"
)


class BrowserFetcher:
    def __init__(
        self,
        headless: bool = True,
        timeout: int = 30000,
        context: BrowserContext | None = None,
        browser: Browser | None = None,
        playwright_instance: Playwright | None = None,
    ) -> None:
        self.headless = headless
        self.timeout = timeout
        self._playwright: Playwright | None = playwright_instance
        self._browser: Browser | None = browser
        self._context: BrowserContext | None = context
        self._page: Page | None = None
        self._external_context: bool = context is not None

    async def _ensure_browser(self) -> Page:
        if self._page is None:
            if self._context is None:
                if self._playwright is None:
                    self._playwright = await async_playwright().start()
                if self._browser is None:
                    self._browser = await self._playwright.chromium.launch(
                        headless=self.headless,
                        args=[
                            "--disable-blink-features=AutomationControlled",
                            "--no-first-run",
                            "--no-default-browser-check",
                        ],
                    )
                self._context = await self._browser.new_context(
                    user_agent=_USER_AGENT,
                    locale="en-US",
                    timezone_id="America/New_York",
                )
            self._page = await self._context.new_page()
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
            async with page.expect_navigation(wait_until="domcontentloaded"):
                await page.evaluate(f'document.querySelector("{selector}").submit()')
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

    async def select_option(
        self,
        selector: str,
        value: str | None = None,
        label: str | None = None,
        index: int | None = None,
    ) -> BrowserAction:
        page = await self._ensure_browser()
        try:
            if value is not None:
                await page.select_option(selector, value=value)
            elif label is not None:
                await page.select_option(selector, label=label)
            elif index is not None:
                await page.select_option(selector, index=index)
            else:
                return BrowserAction(
                    success=False, observation="Provide value, label, or index to select"
                )
            return BrowserAction(
                success=True,
                observation=f"Selected option in '{selector}'",
                new_html=await page.content(),
            )
        except Exception as e:
            return BrowserAction(success=False, observation=f"Select failed on '{selector}': {e}")

    async def go_back(self) -> BrowserAction:
        page = await self._ensure_browser()
        try:
            await page.go_back(wait_until="domcontentloaded")
            return BrowserAction(
                success=True,
                observation=f"Navigated back to {page.url}",
                new_url=page.url,
                new_html=await page.content(),
            )
        except Exception as e:
            return BrowserAction(success=False, observation=f"Go back failed: {e}")

    async def execute_js(self, script: str) -> BrowserAction:
        page = await self._ensure_browser()
        try:
            result = await page.evaluate(script)
            result_str = str(result) if result is not None else "undefined"
            if len(result_str) > 2000:
                result_str = result_str[:2000] + "... [truncated]"
            return BrowserAction(
                success=True,
                observation=f"JS result: {result_str}",
                new_html=await page.content(),
            )
        except Exception as e:
            return BrowserAction(success=False, observation=f"JS execution failed: {e}")

    async def hover(self, selector: str) -> BrowserAction:
        page = await self._ensure_browser()
        try:
            await page.hover(selector, timeout=self.timeout)
            await page.wait_for_timeout(300)
            return BrowserAction(
                success=True,
                observation=f"Hovered over '{selector}'",
                new_html=await page.content(),
            )
        except Exception as e:
            return BrowserAction(success=False, observation=f"Hover failed on '{selector}': {e}")

    async def press_key(self, key: str, selector: str | None = None) -> BrowserAction:
        page = await self._ensure_browser()
        try:
            if selector:
                await page.press(selector, key)
            else:
                await page.keyboard.press(key)
            await page.wait_for_timeout(200)
            return BrowserAction(
                success=True,
                observation=f"Pressed key '{key}'" + (f" on '{selector}'" if selector else ""),
                new_url=page.url,
                new_html=await page.content(),
            )
        except Exception as e:
            return BrowserAction(success=False, observation=f"Key press failed: {e}")

    async def switch_tab(self, index: int = -1) -> BrowserAction:
        """Switch to a browser tab by index. -1 means the last/newest tab."""
        try:
            if self._context is None:
                return BrowserAction(success=False, observation="No browser context available")
            pages = self._context.pages
            if not pages:
                return BrowserAction(success=False, observation="No tabs available")
            target_index = index if index >= 0 else len(pages) + index
            if target_index < 0 or target_index >= len(pages):
                return BrowserAction(
                    success=False,
                    observation=f"Tab index {index} out of range (have {len(pages)} tabs)",
                )
            self._page = pages[target_index]
            await self._page.bring_to_front()
            return BrowserAction(
                success=True,
                observation=f"Switched to tab {target_index} ({self._page.url}). {len(pages)} tab(s) open.",
                new_url=self._page.url,
                new_html=await self._page.content(),
            )
        except Exception as e:
            return BrowserAction(success=False, observation=f"Tab switch failed: {e}")

    async def get_current_html(self) -> str:
        page = await self._ensure_browser()
        return await page.content()

    async def get_current_url(self) -> str:
        page = await self._ensure_browser()
        return page.url

    async def close(self) -> None:
        if self._page:
            await self._page.close()
        if not self._external_context:
            if self._context:
                await self._context.close()
            if self._browser:
                await self._browser.close()
            if self._playwright:
                await self._playwright.stop()
        self._page = None
        self._context = None
        self._browser = None
        self._playwright = None
