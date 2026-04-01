"""Integration tests for the TUI stack — exercises the full module graph without real LLM calls."""

from __future__ import annotations

import pytest
from io import StringIO
from unittest.mock import AsyncMock, MagicMock, patch

from rich.console import Console

from agentic_crawler.tui.display import ReplDisplay
from agentic_crawler.tui.renderer import MarkdownStreamState
from agentic_crawler.tui.tool_display import format_tool_call
from agentic_crawler.tui.session_store import SessionStore, Session
from agentic_crawler.tui.permissions import PermissionPolicy, PermissionMode
from agentic_crawler.tui.repl import ReplLoop


# --------------------------------------------------------------------------- #
# Test 1: All TUI modules import correctly
# --------------------------------------------------------------------------- #


def test_all_tui_modules_importable():
    from agentic_crawler.tui import (
        repl,
        display,
        renderer,
        tool_display,
        session_store,
        permissions,
    )

    assert all([repl, display, renderer, tool_display, session_store, permissions])


# --------------------------------------------------------------------------- #
# Test 2: Full display flow — register, log, done
# --------------------------------------------------------------------------- #


def test_repl_display_full_flow():
    buf = StringIO()
    console = Console(file=buf, force_terminal=False, highlight=False, markup=False)
    display = ReplDisplay(console=console)
    display.register_agent("agent-abc123", "crawl example.com", None, 10)
    display.log_message("agent-abc123", "Starting crawl")
    display.log_step("agent-abc123", 1, "12:00:00", "navigate", "url='https://example.com'")
    display.log_result("agent-abc123", "[green]OK[/green]", "Page loaded")
    display.mark_agent_done("agent-abc123")
    output = buf.getvalue()
    assert len(output) > 0
    assert "crawl example.com" in output
    assert "Starting crawl" in output
    assert "navigate" in output
    assert "Done" in output or "\u2713" in output


# --------------------------------------------------------------------------- #
# Test 3: Streaming markdown through display
# --------------------------------------------------------------------------- #


def test_streaming_text_delta():
    buf = StringIO()
    console = Console(file=buf, force_terminal=False, highlight=False, markup=False)
    display = ReplDisplay(console=console)
    display.register_agent("agent-xyz789", "test", None, 5)
    display.stream_text_delta("agent-xyz789", "Hello world\n\n")
    display.stream_text_delta("agent-xyz789", "More content")
    display.mark_agent_done("agent-xyz789")
    output = buf.getvalue()
    assert "Hello world" in output


# --------------------------------------------------------------------------- #
# Test 4: Session roundtrip
# --------------------------------------------------------------------------- #


def test_session_persistence_roundtrip(tmp_path):
    store = SessionStore(sessions_dir=tmp_path)
    session = Session.create(goal="crawl example.com", settings_snapshot={"provider": "claude"})
    store.save(session)
    loaded = store.load(session.session_id)
    assert loaded.goal == session.goal
    assert loaded.session_id == session.session_id
    assert loaded.settings_snapshot == {"provider": "claude"}


# --------------------------------------------------------------------------- #
# Test 5: Multi-agent interleaved output
# --------------------------------------------------------------------------- #


def test_multi_agent_interleaved_output():
    buf = StringIO()
    console = Console(file=buf, force_terminal=False, highlight=False, markup=False)
    display = ReplDisplay(console=console)
    display.register_agent("agent-111111", "goal 1", None, 5)
    display.register_agent("agent-222222", "goal 2", "agent-111111", 5)
    display.log_message("agent-111111", "Message from agent 1")
    display.log_message("agent-222222", "Message from agent 2")
    display.mark_agent_done("agent-111111")
    display.mark_agent_done("agent-222222")
    output = buf.getvalue()
    assert len(output) > 0
    assert "Message from agent 1" in output
    assert "Message from agent 2" in output


# --------------------------------------------------------------------------- #
# Test 6: Permission policy integration
# --------------------------------------------------------------------------- #


def test_permission_policy_integration():
    policy = PermissionPolicy()
    tools = [
        "navigate",
        "click",
        "fill_form",
        "scroll",
        "extract_data",
        "screenshot",
        "wait",
        "select_option",
        "go_back",
        "execute_js",
        "hover",
        "press_key",
        "switch_tab",
        "list_resources",
        "save_file",
    ]
    for tool in tools:
        assert policy.authorize(tool) is True, f"FullAccess should allow '{tool}'"


def test_permission_tiers_integrated():
    readonly = PermissionPolicy(PermissionMode.ReadOnly)
    workspace = PermissionPolicy(PermissionMode.WorkspaceWrite)
    full = PermissionPolicy(PermissionMode.FullAccess)

    assert readonly.authorize("extract_data") is True
    assert readonly.authorize("save_file") is False
    assert readonly.authorize("navigate") is False

    assert workspace.authorize("extract_data") is True
    assert workspace.authorize("save_file") is True
    assert workspace.authorize("navigate") is False

    assert full.authorize("extract_data") is True
    assert full.authorize("save_file") is True
    assert full.authorize("navigate") is True


# --------------------------------------------------------------------------- #
# Test 7: Tool display in context
# --------------------------------------------------------------------------- #


def test_tool_display_integration():
    output = format_tool_call("navigate", {"url": "https://example.com"}, True, "Page loaded")
    assert "navigate" in output
    assert "\u2713" in output
    assert "\u256d" in output
    assert "\u2570" in output


def test_tool_display_failure_integration():
    output = format_tool_call("click", {"selector": "#btn"}, False, "Element not found")
    assert "click" in output
    assert "\u2717" in output
    assert "Element not found" in output


# --------------------------------------------------------------------------- #
# Test 8: REPL loop with mocked agent — exits on EOFError
# --------------------------------------------------------------------------- #


@pytest.mark.asyncio
async def test_repl_exits_on_eof():
    mock_settings = MagicMock()
    mock_settings.llm_provider = "claude"
    mock_display = MagicMock(spec=ReplDisplay)
    mock_display.get_console.return_value = Console(file=StringIO())

    repl = ReplLoop(settings=mock_settings, display=mock_display)

    with patch("agentic_crawler.tui.repl.PromptSession") as MockSession:
        mock_session = MagicMock()
        mock_session.prompt_async = AsyncMock(side_effect=EOFError())
        MockSession.return_value = mock_session

        await repl.run()


# --------------------------------------------------------------------------- #
# Test 9: REPL dispatches goal to run_agent
# --------------------------------------------------------------------------- #


@pytest.mark.asyncio
async def test_repl_dispatches_goal_to_agent():
    mock_settings = MagicMock()
    mock_settings.llm_provider = "claude"
    mock_display = MagicMock(spec=ReplDisplay)
    mock_display.get_console.return_value = Console(file=StringIO())

    repl = ReplLoop(settings=mock_settings, display=mock_display)

    with patch("agentic_crawler.tui.repl.PromptSession") as MockSession:
        mock_session = MagicMock()
        mock_session.prompt_async = AsyncMock(side_effect=["scrape example.com", EOFError()])
        MockSession.return_value = mock_session

        with patch("agentic_crawler.tui.repl.run_agent", new_callable=AsyncMock) as mock_run:
            await repl.run()
            mock_run.assert_called_once_with(
                goal="scrape example.com",
                settings=mock_settings,
                display=mock_display,
            )


# --------------------------------------------------------------------------- #
# Test 10: Renderer state machine — push/flush lifecycle
# --------------------------------------------------------------------------- #


def test_renderer_push_flush_lifecycle():
    state = MarkdownStreamState()

    result = state.push("Hello ")
    assert result is None

    result = state.push("world\n\n")
    assert result is not None
    assert "Hello" in result

    state.push("remaining text")
    flushed = state.flush()
    assert flushed is not None
    assert "remaining" in flushed

    assert state.flush() is None


# --------------------------------------------------------------------------- #
# Test 11: Session list and delete integration
# --------------------------------------------------------------------------- #


def test_session_list_and_delete(tmp_path):
    store = SessionStore(sessions_dir=tmp_path)

    s1 = Session.create(goal="goal 1")
    s2 = Session.create(goal="goal 2")
    store.save(s1)
    store.save(s2)

    summaries = store.list_sessions()
    assert len(summaries) == 2
    ids = {s.session_id for s in summaries}
    assert s1.session_id in ids
    assert s2.session_id in ids

    store.delete(s1.session_id)
    summaries_after = store.list_sessions()
    assert len(summaries_after) == 1
    assert summaries_after[0].session_id == s2.session_id


# --------------------------------------------------------------------------- #
# Test 12: Display ignores unregistered agents gracefully
# --------------------------------------------------------------------------- #


def test_display_ignores_unregistered_agent():
    buf = StringIO()
    console = Console(file=buf, force_terminal=False, highlight=False, markup=False)
    display = ReplDisplay(console=console)
    display.log_message("ghost", "should be ignored")
    display.log_step("ghost", 1, "00:00", "navigate", "url=x")
    display.log_result("ghost", "[green]OK[/green]", "ok")
    display.stream_text_delta("ghost", "text")
    display.set_thinking("ghost", True)
    display.mark_agent_done("ghost")
    output = buf.getvalue()
    assert "should be ignored" not in output
