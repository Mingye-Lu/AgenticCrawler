from __future__ import annotations

import io

from rich.console import Console
from rich.markdown import Markdown

from agentic_crawler.agent.display import ConsoleDisplay


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
