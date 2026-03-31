from __future__ import annotations

from unittest.mock import AsyncMock, MagicMock, patch

import pytest

from agentic_crawler.fetcher.browser_fetcher import BrowserFetcher


@pytest.mark.asyncio
async def test_browser_fetcher_creates_own_context_by_default() -> None:
    mock_manager = MagicMock()
    mock_playwright = MagicMock()
    mock_browser = AsyncMock()
    mock_context = AsyncMock()
    mock_page = MagicMock()
    mock_page.close = AsyncMock()

    mock_manager.start = AsyncMock(return_value=mock_playwright)
    mock_playwright.chromium.launch = AsyncMock(return_value=mock_browser)
    mock_playwright.stop = AsyncMock()
    mock_browser.new_context = AsyncMock(return_value=mock_context)
    mock_context.new_page = AsyncMock(return_value=mock_page)

    with patch(
        "agentic_crawler.fetcher.browser_fetcher.async_playwright", return_value=mock_manager
    ):
        fetcher = BrowserFetcher()

        page = await fetcher._ensure_browser()

        assert page is mock_page
        mock_manager.start.assert_awaited_once()
        mock_playwright.chromium.launch.assert_awaited_once()
        mock_browser.new_context.assert_awaited_once()
        mock_context.new_page.assert_awaited_once()
        mock_page.set_default_timeout.assert_called_once_with(30000)

        await fetcher.close()

    mock_page.close.assert_awaited_once()
    mock_context.close.assert_awaited_once()
    mock_browser.close.assert_awaited_once()
    mock_playwright.stop.assert_awaited_once()


@pytest.mark.asyncio
async def test_browser_fetcher_accepts_external_context() -> None:
    mock_context = AsyncMock()
    mock_page = MagicMock()
    mock_page.close = AsyncMock()
    mock_context.new_page = AsyncMock(return_value=mock_page)

    with patch("agentic_crawler.fetcher.browser_fetcher.async_playwright") as mock_async_playwright:
        fetcher = BrowserFetcher(context=mock_context)

        page = await fetcher._ensure_browser()

    assert page is mock_page
    mock_context.new_page.assert_awaited_once()
    mock_page.set_default_timeout.assert_called_once_with(30000)
    mock_async_playwright.assert_not_called()


@pytest.mark.asyncio
async def test_browser_fetcher_close_with_external_context_only_closes_page() -> None:
    mock_context = AsyncMock()
    mock_page = MagicMock()
    mock_page.close = AsyncMock()
    mock_context.new_page = AsyncMock(return_value=mock_page)
    mock_context.close = AsyncMock()

    mock_browser = AsyncMock()
    mock_playwright = AsyncMock()

    fetcher = BrowserFetcher(
        context=mock_context,
        browser=mock_browser,
        playwright_instance=mock_playwright,
    )
    await fetcher._ensure_browser()

    await fetcher.close()

    mock_page.close.assert_awaited_once()
    mock_context.close.assert_not_awaited()
    mock_browser.close.assert_not_awaited()
    mock_playwright.stop.assert_not_awaited()


@pytest.mark.asyncio
async def test_browser_fetcher_close_without_external_context_closes_all() -> None:
    fetcher = BrowserFetcher()
    page = AsyncMock()
    context = AsyncMock()
    browser = AsyncMock()
    playwright = AsyncMock()

    fetcher._page = page
    fetcher._context = context
    fetcher._browser = browser
    fetcher._playwright = playwright
    fetcher._external_context = False

    await fetcher.close()

    page.close.assert_awaited_once()
    context.close.assert_awaited_once()
    browser.close.assert_awaited_once()
    playwright.stop.assert_awaited_once()
