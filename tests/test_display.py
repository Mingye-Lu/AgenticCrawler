from __future__ import annotations

import io

from rich.console import Console
from rich.markdown import Markdown

from agentic_crawler.agent.display import ConsoleDisplay
from agentic_crawler.agent.display import LiveDashboard


def _make_display(
    verbose: bool = False, is_root: bool = True, agent_id: str = ""
) -> tuple[ConsoleDisplay, io.StringIO]:
    buf = io.StringIO()
    console = Console(file=buf, force_terminal=False, highlight=False)
    display = ConsoleDisplay(console=console, verbose=verbose, agent_id=agent_id, is_root=is_root)
    return display, buf


def _text(buf: io.StringIO) -> str:
    return buf.getvalue()


def test_log_message_root_prefix() -> None:
    display, buf = _make_display(is_root=True)
    display.log_message("any-id", "hello world")
    assert "hello world" in _text(buf)
    assert "[root]" in display._prefix


def test_log_message_fork_prefix() -> None:
    display, buf = _make_display(is_root=False, agent_id="fork-abcdef12")
    display.log_message("fork-abcdef12", "hello fork")
    assert "hello fork" in _text(buf)
    assert "fork-a" in display._prefix


def test_log_step_contains_step_label_and_action() -> None:
    display, buf = _make_display()
    display.log_step("root", 3, "12:34:56", "navigate", "url=https://example.com")
    out = _text(buf)
    assert "Step 3" in out
    assert "12:34:56" in out
    assert "navigate" in out
    assert "url=https://example.com" in out


def test_log_step_prefix_present() -> None:
    display, buf = _make_display(is_root=True)
    display.log_step("root", 1, "00:00:00", "click", "selector=.btn")
    assert "Step 1" in _text(buf)
    assert "[root]" in display._prefix


def test_log_result_verbose_includes_observation() -> None:
    display, buf = _make_display(verbose=True)
    display.log_result("root", "[green]OK[/green]", "Page loaded successfully")
    out = _text(buf)
    assert "OK" in out
    assert "Page loaded successfully" in out


def test_log_result_compact_omits_observation() -> None:
    display, buf = _make_display(verbose=False)
    display.log_result("root", "[green]OK[/green]", "Page loaded successfully")
    out = _text(buf)
    assert "OK" in out
    assert "Page loaded successfully" not in out


def test_log_result_compact_no_observation_arg() -> None:
    display, buf = _make_display(verbose=False)
    display.log_result("root", "[red]FAIL[/red]", None)
    out = _text(buf)
    assert "FAIL" in out


def test_print_panel_with_style() -> None:
    display, buf = _make_display(is_root=True)
    display.print_panel("root", "ignored-title", "[bold]Planning...[/bold]", "blue")
    assert "Planning..." in _text(buf)
    assert "[root]" in display._prefix


def test_print_panel_without_style() -> None:
    display, buf = _make_display(is_root=True)
    display.print_panel("root", "ignored-title", "[bold green]Done![/bold green] 3 item(s)", "")
    out = _text(buf)
    assert "Done!" in out


def test_set_thinking_true_prints_header() -> None:
    display, buf = _make_display(is_root=True)
    display.set_thinking("root", True)
    out = _text(buf)
    assert "Thinking..." in out


def test_set_thinking_true_only_once() -> None:
    display, buf = _make_display(is_root=True)
    display.set_thinking("root", True)
    display.set_thinking("root", True)
    out = _text(buf)
    assert out.count("Thinking...") == 1


def test_set_thinking_false_writes_newline_to_file() -> None:
    display, buf = _make_display(is_root=True)
    display.set_thinking("root", True)
    before = _text(buf)
    display.set_thinking("root", False)
    after = _text(buf)
    assert after != before
    assert after.endswith("\n")


def test_set_thinking_false_without_prior_true_is_noop() -> None:
    display, buf = _make_display(is_root=True)
    display.set_thinking("root", False)
    assert _text(buf) == ""


def test_stream_thinking_chunk_writes_to_file() -> None:
    display, buf = _make_display(is_root=True)
    display.stream_thinking_chunk("root", "some chunk")
    assert "some chunk" in _text(buf)


def test_register_agent_does_not_raise() -> None:
    display, _ = _make_display()
    display.register_agent("fork-abc123", "some goal", None, 20)
    display.register_agent("fork-xyz456", "child goal", "fork-abc123", 10)


def test_mark_agent_done_does_not_raise() -> None:
    display, _ = _make_display()
    display.mark_agent_done("root")
    display.mark_agent_done("fork-abc123")


def test_print_final_output_string() -> None:
    display, buf = _make_display()
    display.print_final_output("Final result text")
    assert "Final result text" in _text(buf)


def test_print_final_output_renderable() -> None:
    display, buf = _make_display()
    md = Markdown("# Title\nContent here")
    display.print_final_output(md)
    out = _text(buf)
    assert "Title" in out
    assert "Content here" in out


def test_get_console_returns_same_instance() -> None:
    buf = io.StringIO()
    console = Console(file=buf, force_terminal=False)
    display = ConsoleDisplay(console=console)
    assert display.get_console() is console


def test_verbose_attribute_stored() -> None:
    display, _ = _make_display(verbose=True)
    assert display.verbose is True


def test_non_verbose_attribute_stored() -> None:
    display, _ = _make_display(verbose=False)
    assert display.verbose is False


def test_root_prefix_for_non_fork_id() -> None:
    display, _ = _make_display(is_root=True, agent_id="some-uuid-string")
    assert display._prefix == "[bold dim][root][/bold dim]"


def test_fork_prefix_uses_first_six_chars() -> None:
    display, _ = _make_display(is_root=False, agent_id="fork-ab1234cd")
    assert "fork-a" in display._prefix


def test_fork_prefix_short_id_under_six_chars() -> None:
    display, _ = _make_display(is_root=False, agent_id="abc")
    assert "abc" in display._prefix


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
