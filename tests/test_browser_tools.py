"""
TDD tests for all browser tools rendered in a real Playwright browser (headless=False).

Each test launches a visible browser, serves a local HTML fixture, and exercises
the corresponding Action through a real FetcherRouter.
"""
from __future__ import annotations

import asyncio
import base64
import threading
from http.server import HTTPServer, SimpleHTTPRequestHandler
from typing import Any, Generator

import pytest

from agentic_crawler.actions.click import ClickAction
from agentic_crawler.actions.execute_js import ExecuteJsAction
from agentic_crawler.actions.extract import ExtractDataAction
from agentic_crawler.actions.fill_form import FillFormAction
from agentic_crawler.actions.go_back import GoBackAction
from agentic_crawler.actions.hover import HoverAction
from agentic_crawler.actions.navigate import NavigateAction
from agentic_crawler.actions.press_key import PressKeyAction
from agentic_crawler.actions.screenshot import ScreenshotAction
from agentic_crawler.actions.scroll import ScrollAction
from agentic_crawler.actions.select_option import SelectOptionAction
from agentic_crawler.actions.switch_tab import SwitchTabAction
from agentic_crawler.actions.wait import WaitAction
from agentic_crawler.fetcher.router import FetcherRouter

# ---------------------------------------------------------------------------
# Test HTML fixture served by a local HTTP server
# ---------------------------------------------------------------------------

TEST_HTML = """\
<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8"><title>Tool Test Page</title>
<style>
  body { font-family: sans-serif; padding: 20px; min-height: 2000px; }
  .hidden-tooltip { display: none; }
  .hoverable:hover + .hidden-tooltip { display: block; }
  #js-result { color: green; }
</style>
</head>
<body>
  <h1 id="heading">Tool Test Page</h1>

  <!-- navigate / click -->
  <a id="about-link" href="/about">About</a>
  <button id="btn" onclick="document.getElementById('btn-output').textContent='clicked'">Click Me</button>
  <span id="btn-output"></span>

  <!-- fill_form -->
  <form id="search-form" action="/search" method="get">
    <input id="q" type="text" name="q" placeholder="Search...">
    <select id="color-select" name="color">
      <option value="red">Red</option>
      <option value="green">Green</option>
      <option value="blue">Blue</option>
    </select>
    <button type="submit">Search</button>
  </form>

  <!-- hover tooltip -->
  <span class="hoverable" id="hover-target">Hover me</span>
  <span class="hidden-tooltip" id="tooltip">Tooltip visible!</span>

  <!-- press_key target -->
  <input id="key-input" type="text" placeholder="press keys here">

  <!-- js result area -->
  <div id="js-result"></div>

  <!-- long content for scrolling -->
  <div style="margin-top: 1500px;" id="bottom-marker">Bottom of Page</div>
</body>
</html>
"""

ABOUT_HTML = """\
<!DOCTYPE html>
<html><head><title>About</title></head>
<body><h1 id="about-heading">About Page</h1><a href="/">Home</a></body>
</html>
"""

SEARCH_HTML = """\
<!DOCTYPE html>
<html><head><title>Search Results</title></head>
<body><h1 id="search-heading">Search Results</h1><p id="query"></p></body>
</html>
"""


class _TestHandler(SimpleHTTPRequestHandler):
    """Serves the test HTML fixtures."""

    def do_GET(self) -> None:
        if self.path == "/" or self.path.startswith("/?"):
            self._respond(TEST_HTML)
        elif self.path == "/about":
            self._respond(ABOUT_HTML)
        elif self.path.startswith("/search"):
            self._respond(SEARCH_HTML)
        else:
            self._respond(TEST_HTML)

    def _respond(self, html: str) -> None:
        body = html.encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: Any) -> None:
        pass  # silence request logs


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture(scope="module")
def local_server() -> Generator[str, None, None]:
    """Start a local HTTP server in a background thread and yield its base URL."""
    server = HTTPServer(("127.0.0.1", 0), _TestHandler)
    port = server.server_address[1]
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    yield f"http://127.0.0.1:{port}"
    server.shutdown()


@pytest.fixture
async def router() -> FetcherRouter:
    """Create a FetcherRouter with headless=False for visible browser testing."""
    r = FetcherRouter(headless=False, browser_timeout=10000)
    yield r  # type: ignore[misc]
    await r.close()


# ---------------------------------------------------------------------------
# Tests — Navigate
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_navigate_action(local_server: str, router: FetcherRouter) -> None:
    action = NavigateAction()
    result = await action.execute(router, {"url": local_server})
    assert result.success
    assert "Tool Test Page" in (result.new_html or "")
    assert local_server.rstrip("/") in (result.new_url or "")


@pytest.mark.asyncio
async def test_navigate_no_url(router: FetcherRouter) -> None:
    action = NavigateAction()
    result = await action.execute(router, {})
    assert not result.success


# ---------------------------------------------------------------------------
# Tests — Click
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_click_by_selector(local_server: str, router: FetcherRouter) -> None:
    nav = NavigateAction()
    await nav.execute(router, {"url": local_server})

    action = ClickAction()
    result = await action.execute(router, {"selector": "#btn"})
    assert result.success
    assert "clicked" in (result.new_html or "").lower() or "Clicked" in result.observation


@pytest.mark.asyncio
async def test_click_by_text(local_server: str, router: FetcherRouter) -> None:
    nav = NavigateAction()
    await nav.execute(router, {"url": local_server})

    action = ClickAction()
    result = await action.execute(router, {"text": "Click Me"})
    assert result.success


@pytest.mark.asyncio
async def test_click_link_navigates(local_server: str, router: FetcherRouter) -> None:
    nav = NavigateAction()
    await nav.execute(router, {"url": local_server})

    action = ClickAction()
    result = await action.execute(router, {"selector": "#about-link"})
    assert result.success
    assert "/about" in (result.new_url or "")


@pytest.mark.asyncio
async def test_click_no_params(router: FetcherRouter) -> None:
    action = ClickAction()
    result = await action.execute(router, {})
    assert not result.success


# ---------------------------------------------------------------------------
# Tests — Fill Form
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_fill_form_fields(local_server: str, router: FetcherRouter) -> None:
    nav = NavigateAction()
    await nav.execute(router, {"url": local_server})

    action = FillFormAction()
    result = await action.execute(router, {
        "fields": {"#q": "playwright test"},
    })
    assert result.success
    assert "Filled" in result.observation


@pytest.mark.asyncio
async def test_fill_form_and_submit(local_server: str, router: FetcherRouter) -> None:
    nav = NavigateAction()
    await nav.execute(router, {"url": local_server})

    action = FillFormAction()
    result = await action.execute(router, {
        "fields": {"#q": "hello"},
        "submit": True,
        "form_selector": "#search-form",
    })
    assert result.success
    assert "/search" in (result.new_url or "")


@pytest.mark.asyncio
async def test_fill_form_no_fields(router: FetcherRouter) -> None:
    action = FillFormAction()
    result = await action.execute(router, {})
    assert not result.success


# ---------------------------------------------------------------------------
# Tests — Scroll
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_scroll_down(local_server: str, router: FetcherRouter) -> None:
    nav = NavigateAction()
    await nav.execute(router, {"url": local_server})

    action = ScrollAction()
    result = await action.execute(router, {"direction": "down", "amount": 500})
    assert result.success
    assert "down" in result.observation.lower()


@pytest.mark.asyncio
async def test_scroll_up(local_server: str, router: FetcherRouter) -> None:
    nav = NavigateAction()
    await nav.execute(router, {"url": local_server})

    # scroll down first, then up
    action = ScrollAction()
    await action.execute(router, {"direction": "down", "amount": 500})
    result = await action.execute(router, {"direction": "up", "amount": 300})
    assert result.success
    assert "up" in result.observation.lower()


# ---------------------------------------------------------------------------
# Tests — Screenshot
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_screenshot(local_server: str, router: FetcherRouter) -> None:
    nav = NavigateAction()
    await nav.execute(router, {"url": local_server})

    action = ScreenshotAction()
    result = await action.execute(router, {})
    assert result.success
    assert result.data is not None
    # Verify it's valid base64-encoded PNG
    png_bytes = base64.b64decode(result.data["screenshot_base64"])
    assert png_bytes[:4] == b"\x89PNG"


# ---------------------------------------------------------------------------
# Tests — Wait
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_wait_for_selector(local_server: str, router: FetcherRouter) -> None:
    nav = NavigateAction()
    await nav.execute(router, {"url": local_server})

    action = WaitAction()
    result = await action.execute(router, {"selector": "#heading"})
    assert result.success
    assert "appeared" in result.observation.lower()


@pytest.mark.asyncio
async def test_wait_timeout(local_server: str, router: FetcherRouter) -> None:
    nav = NavigateAction()
    await nav.execute(router, {"url": local_server})

    action = WaitAction()
    result = await action.execute(router, {"seconds": 0.5})
    assert result.success
    assert "Waited" in result.observation


@pytest.mark.asyncio
async def test_wait_for_missing_selector(local_server: str, router: FetcherRouter) -> None:
    nav = NavigateAction()
    await nav.execute(router, {"url": local_server})

    action = WaitAction()
    result = await action.execute(router, {"selector": "#nonexistent", "seconds": 1})
    assert not result.success


# ---------------------------------------------------------------------------
# Tests — Select Option
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_select_option_by_value(local_server: str, router: FetcherRouter) -> None:
    nav = NavigateAction()
    await nav.execute(router, {"url": local_server})

    action = SelectOptionAction()
    result = await action.execute(router, {"selector": "#color-select", "value": "blue"})
    assert result.success
    assert "Selected" in result.observation


@pytest.mark.asyncio
async def test_select_option_by_label(local_server: str, router: FetcherRouter) -> None:
    nav = NavigateAction()
    await nav.execute(router, {"url": local_server})

    action = SelectOptionAction()
    result = await action.execute(router, {"selector": "#color-select", "label": "Green"})
    assert result.success


@pytest.mark.asyncio
async def test_select_option_by_index(local_server: str, router: FetcherRouter) -> None:
    nav = NavigateAction()
    await nav.execute(router, {"url": local_server})

    action = SelectOptionAction()
    result = await action.execute(router, {"selector": "#color-select", "index": 0})
    assert result.success


@pytest.mark.asyncio
async def test_select_option_no_selector(router: FetcherRouter) -> None:
    action = SelectOptionAction()
    result = await action.execute(router, {})
    assert not result.success


@pytest.mark.asyncio
async def test_select_option_no_value(router: FetcherRouter) -> None:
    action = SelectOptionAction()
    result = await action.execute(router, {"selector": "#color-select"})
    assert not result.success


# ---------------------------------------------------------------------------
# Tests — Go Back
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_go_back(local_server: str, router: FetcherRouter) -> None:
    nav = NavigateAction()
    await nav.execute(router, {"url": local_server})
    await nav.execute(router, {"url": f"{local_server}/about"})

    action = GoBackAction()
    result = await action.execute(router, {})
    assert result.success
    # Should go back to the main page
    assert "/about" not in (result.new_url or "")


# ---------------------------------------------------------------------------
# Tests — Execute JS
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_execute_js(local_server: str, router: FetcherRouter) -> None:
    nav = NavigateAction()
    await nav.execute(router, {"url": local_server})

    action = ExecuteJsAction()
    result = await action.execute(router, {"script": "document.title"})
    assert result.success
    assert "Tool Test Page" in result.observation


@pytest.mark.asyncio
async def test_execute_js_modifies_dom(local_server: str, router: FetcherRouter) -> None:
    nav = NavigateAction()
    await nav.execute(router, {"url": local_server})

    action = ExecuteJsAction()
    result = await action.execute(router, {
        "script": "document.getElementById('js-result').textContent = 'JS was here'; 'done'"
    })
    assert result.success
    assert "JS was here" in (result.new_html or "")


@pytest.mark.asyncio
async def test_execute_js_no_script(router: FetcherRouter) -> None:
    action = ExecuteJsAction()
    result = await action.execute(router, {})
    assert not result.success


# ---------------------------------------------------------------------------
# Tests — Hover
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_hover(local_server: str, router: FetcherRouter) -> None:
    nav = NavigateAction()
    await nav.execute(router, {"url": local_server})

    action = HoverAction()
    result = await action.execute(router, {"selector": "#hover-target"})
    assert result.success
    assert "Hovered" in result.observation


@pytest.mark.asyncio
async def test_hover_no_selector(router: FetcherRouter) -> None:
    action = HoverAction()
    result = await action.execute(router, {})
    assert not result.success


# ---------------------------------------------------------------------------
# Tests — Press Key
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_press_key_on_element(local_server: str, router: FetcherRouter) -> None:
    nav = NavigateAction()
    await nav.execute(router, {"url": local_server})

    # Fill the input first, then press a key on it
    fill = FillFormAction()
    await fill.execute(router, {"fields": {"#key-input": "hello"}})

    action = PressKeyAction()
    result = await action.execute(router, {"key": "Enter", "selector": "#key-input"})
    assert result.success
    assert "Enter" in result.observation


@pytest.mark.asyncio
async def test_press_key_global(local_server: str, router: FetcherRouter) -> None:
    nav = NavigateAction()
    await nav.execute(router, {"url": local_server})

    action = PressKeyAction()
    result = await action.execute(router, {"key": "Escape"})
    assert result.success
    assert "Escape" in result.observation


@pytest.mark.asyncio
async def test_press_key_no_key(router: FetcherRouter) -> None:
    action = PressKeyAction()
    result = await action.execute(router, {})
    assert not result.success


# ---------------------------------------------------------------------------
# Tests — Switch Tab
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_switch_tab(local_server: str, router: FetcherRouter) -> None:
    nav = NavigateAction()
    await nav.execute(router, {"url": local_server})

    # Open a new tab via JS
    js = ExecuteJsAction()
    await js.execute(router, {"script": f"window.open('{local_server}/about', '_blank')"})
    # Small wait for new tab
    await asyncio.sleep(0.5)

    action = SwitchTabAction()
    # Switch to the new (last) tab
    result = await action.execute(router, {"index": -1})
    assert result.success
    assert "/about" in (result.new_url or "") or "About" in (result.new_html or "")

    # Switch back to first tab
    result = await action.execute(router, {"index": 0})
    assert result.success
    assert "Tool Test Page" in (result.new_html or "")


@pytest.mark.asyncio
async def test_switch_tab_out_of_range(local_server: str, router: FetcherRouter) -> None:
    nav = NavigateAction()
    await nav.execute(router, {"url": local_server})

    action = SwitchTabAction()
    result = await action.execute(router, {"index": 99})
    assert not result.success
    assert "out of range" in result.observation.lower()


# ---------------------------------------------------------------------------
# Tests — Extract Data (does not need browser, but test it anyway)
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_extract_data(router: FetcherRouter) -> None:
    action = ExtractDataAction()
    result = await action.execute(router, {
        "instruction": "Extract product names",
        "data": [{"name": "Widget A"}, {"name": "Widget B"}],
    })
    assert result.success
    assert len(result.data) == 2


@pytest.mark.asyncio
async def test_extract_data_no_params(router: FetcherRouter) -> None:
    action = ExtractDataAction()
    result = await action.execute(router, {})
    assert not result.success
