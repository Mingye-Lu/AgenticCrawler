from __future__ import annotations

import io

from rich.console import Console

from agentic_crawler.agent.display import LiveDashboard


# ────────────────────────────────────────────
# LiveDashboard tests
# ────────────────────────────────────────────


def _live_dashboard() -> LiveDashboard:
    return LiveDashboard(
        console=Console(file=io.StringIO(), force_terminal=False),
        verbose=False,
    )


def test_live_dashboard_register_agent_creates_panel_state() -> None:
    dash = _live_dashboard()
    dash.register_agent("agent-1", "goal one", None, 10)
    dash.register_agent("agent-2", "goal two", "agent-1", 5)
    assert len(dash._agents) == 2
    assert dash._agents["agent-1"].goal == "goal one"
    assert dash._agents["agent-2"].parent_id == "agent-1"


def test_live_dashboard_log_step_updates_rolling_log() -> None:
    dash = _live_dashboard()
    dash.register_agent("agent-1", "test goal", None, 10)
    for i in range(1, 8):
        dash.log_step("agent-1", i, "12:00:00", "navigate", f"url=http://example.com/{i}")
    assert len(dash._agents["agent-1"].last_steps) == 5
    assert "http://example.com/7" in dash._agents["agent-1"].last_steps[-1]


def test_live_dashboard_set_thinking_true_changes_status() -> None:
    dash = _live_dashboard()
    dash.register_agent("agent-1", "test", None, 10)
    dash.set_thinking("agent-1", True)
    assert dash._agents["agent-1"].status == "Thinking..."


def test_live_dashboard_set_thinking_false_reverts_status() -> None:
    dash = _live_dashboard()
    dash.register_agent("agent-1", "test", None, 10)
    dash.set_thinking("agent-1", True)
    dash.set_thinking("agent-1", False)
    assert "Running" in dash._agents["agent-1"].status


def test_live_dashboard_stream_thinking_chunk_is_noop() -> None:
    dash = _live_dashboard()
    dash.register_agent("agent-1", "test", None, 10)
    dash.stream_thinking_chunk("agent-1", "some chunk")


def test_live_dashboard_mark_agent_done_updates_status() -> None:
    dash = _live_dashboard()
    dash.register_agent("agent-1", "test", None, 10)
    dash.mark_agent_done("agent-1")
    assert dash._agents["agent-1"].status == "Done"


def test_live_dashboard_auto_stop_when_all_done() -> None:
    dash = _live_dashboard()
    dash.register_agent("agent-1", "goal 1", None, 10)
    dash.register_agent("agent-2", "goal 2", "agent-1", 5)
    dash.start()
    assert dash._live is not None
    dash.mark_agent_done("agent-1")
    assert dash._live is not None
    dash.mark_agent_done("agent-2")
    assert dash._live is None


def test_live_dashboard_buffered_output_flushed_on_stop() -> None:
    buf = io.StringIO()
    con = Console(file=buf, force_terminal=False)
    dash = LiveDashboard(console=con)
    dash.register_agent("agent-1", "goal", None, 10)
    dash.print_final_output("final summary text")
    assert "final summary text" not in buf.getvalue()
    dash.start()
    dash.mark_agent_done("agent-1")
    assert "final summary text" in buf.getvalue()


def test_live_dashboard_build_renderable_has_panels() -> None:
    dash = _live_dashboard()
    dash.register_agent("agent-1", "goal for agent 1", None, 10)
    renderable = dash._build_renderable()
    assert renderable is not None


def test_live_dashboard_unknown_agent_log_step_no_crash() -> None:
    dash = _live_dashboard()
    dash.log_step("nonexistent", 1, "12:00:00", "navigate", "url=http://example.com")


def test_live_dashboard_get_console_returns_console() -> None:
    con = Console(file=io.StringIO(), force_terminal=False)
    dash = LiveDashboard(console=con)
    assert dash.get_console() is con
