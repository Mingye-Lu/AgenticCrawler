from __future__ import annotations

import inspect
from io import StringIO
from unittest.mock import AsyncMock, MagicMock, patch

import pytest
from rich.console import Console

from agentic_crawler.tui.repl import HISTORY_FILE, PROMPT, ReplLoop


@pytest.fixture
def mock_settings():
    s = MagicMock()
    s.llm_provider = "claude"
    s.max_steps = 5
    return s


@pytest.fixture
def mock_display():
    d = MagicMock()
    d.get_console.return_value = Console(file=StringIO())
    return d


@pytest.fixture
def repl(mock_settings, mock_display):
    return ReplLoop(settings=mock_settings, display=mock_display)


def test_repl_loop_instantiation(mock_settings, mock_display):
    loop = ReplLoop(settings=mock_settings, display=mock_display)
    assert loop.settings is mock_settings
    assert loop.display is mock_display
    assert loop._history_file == HISTORY_FILE


def test_run_is_coroutine(repl):
    assert inspect.iscoroutinefunction(repl.run)


@pytest.mark.asyncio
async def test_empty_input_ignored(repl):
    mock_session = MagicMock()
    mock_session.prompt_async = AsyncMock(side_effect=["", "   ", EOFError()])

    with patch("agentic_crawler.tui.repl.PromptSession", return_value=mock_session):
        with patch("agentic_crawler.tui.repl.run_agent", new_callable=AsyncMock) as mock_run:
            await repl.run()
            mock_run.assert_not_called()


@pytest.mark.asyncio
async def test_non_empty_input_calls_run_agent(repl, mock_settings, mock_display):
    mock_session = MagicMock()
    mock_session.prompt_async = AsyncMock(side_effect=["scrape example.com", EOFError()])

    with patch("agentic_crawler.tui.repl.PromptSession", return_value=mock_session):
        with patch("agentic_crawler.tui.repl.run_agent", new_callable=AsyncMock) as mock_run:
            await repl.run()
            mock_run.assert_called_once_with(
                goal="scrape example.com",
                settings=mock_settings,
                display=mock_display,
            )


@pytest.mark.asyncio
async def test_eof_exits_cleanly(repl):
    mock_session = MagicMock()
    mock_session.prompt_async = AsyncMock(side_effect=EOFError())

    with patch("agentic_crawler.tui.repl.PromptSession", return_value=mock_session):
        await repl.run()


@pytest.mark.asyncio
async def test_keyboard_interrupt_continues(repl):
    mock_session = MagicMock()
    mock_session.prompt_async = AsyncMock(side_effect=[KeyboardInterrupt(), "hello", EOFError()])

    with patch("agentic_crawler.tui.repl.PromptSession", return_value=mock_session):
        with patch("agentic_crawler.tui.repl.run_agent", new_callable=AsyncMock) as mock_run:
            await repl.run()
            mock_run.assert_called_once()


@pytest.mark.asyncio
async def test_run_agent_exception_printed(repl):
    mock_session = MagicMock()
    mock_session.prompt_async = AsyncMock(side_effect=["go", EOFError()])

    with patch("agentic_crawler.tui.repl.PromptSession", return_value=mock_session):
        with patch(
            "agentic_crawler.tui.repl.run_agent",
            new_callable=AsyncMock,
            side_effect=RuntimeError("boom"),
        ):
            await repl.run()

    output = repl.display.get_console().file.getvalue()
    assert "Error" in output
    assert "boom" in output


def test_print_banner_includes_provider(repl):
    repl._print_banner()
    output = repl.display.get_console().file.getvalue()
    assert "AgenticCrawler" in output
    assert "claude" in output


def test_prompt_constant():
    assert isinstance(PROMPT, str)
    assert len(PROMPT) > 0
