from __future__ import annotations

import threading
from io import StringIO

from rich.console import Console
from rich.text import Text

from agentic_crawler.tui.display import ReplDisplay


def _make_display(verbose: bool = False) -> tuple[ReplDisplay, StringIO]:
    buf = StringIO()
    console = Console(file=buf, highlight=False, markup=False, force_terminal=False)
    return ReplDisplay(console, verbose=verbose), buf


class TestProtocolConformance:
    def test_all_protocol_methods_exist(self) -> None:
        required = [
            "print_panel",
            "log_step",
            "log_result",
            "log_message",
            "set_thinking",
            "stream_thinking_chunk",
            "print_final_output",
            "register_agent",
            "mark_agent_done",
            "get_console",
        ]
        disp, _ = _make_display()
        for name in required:
            assert hasattr(disp, name), f"Missing method: {name}"
            assert callable(getattr(disp, name)), f"Not callable: {name}"


class TestRegisterAgent:
    def test_stores_agent(self) -> None:
        disp, _ = _make_display()
        disp.register_agent("agent-abc123", "scrape site", None, 50)
        assert "agent-abc123" in disp._agents

    def test_prints_goal_header(self) -> None:
        disp, buf = _make_display()
        disp.register_agent("agent-abc123", "scrape site", None, 50)
        output = buf.getvalue()
        assert "agent-" in output
        assert "scrape site" in output


class TestLogMessage:
    def test_output_contains_agent_prefix(self) -> None:
        disp, buf = _make_display()
        disp.register_agent("agent-xyz789", "test goal", None, 10)
        disp.log_message("agent-xyz789", "hello world")
        output = buf.getvalue()
        assert "[agent-]" in output
        assert "hello world" in output

    def test_strips_rich_markup(self) -> None:
        disp, buf = _make_display()
        disp.register_agent("agent-xyz789", "test goal", None, 10)
        disp.log_message("agent-xyz789", "[bold red]warning[/bold red]")
        output = buf.getvalue()
        assert "warning" in output


class TestLogStep:
    def test_output_contains_step_number(self) -> None:
        disp, buf = _make_display()
        disp.register_agent("agent-abc123", "goal", None, 50)
        disp.log_step("agent-abc123", 3, "12:00:00", "navigate", "url='http://x.com'")
        output = buf.getvalue()
        assert "3" in output
        assert "navigate" in output


class TestStreamTextDelta:
    def test_push_returns_none_for_partial(self) -> None:
        disp, buf = _make_display()
        disp.register_agent("agent-abc123", "goal", None, 50)
        disp.stream_text_delta("agent-abc123", "hello")
        output_after_reg = buf.getvalue()
        disp.stream_text_delta("agent-abc123", " world")
        output_after_push = buf.getvalue()
        delta = output_after_push[len(output_after_reg) :]
        assert "hello" not in delta

    def test_renders_at_boundary(self) -> None:
        disp, buf = _make_display()
        disp.register_agent("agent-abc123", "goal", None, 50)
        pre = buf.getvalue()
        disp.stream_text_delta("agent-abc123", "paragraph one\n\n")
        post = buf.getvalue()
        delta = post[len(pre) :]
        assert "paragraph one" in delta


class TestMarkAgentDone:
    def test_prints_done_message(self) -> None:
        disp, buf = _make_display()
        disp.register_agent("agent-abc123", "goal", None, 50)
        pre = buf.getvalue()
        disp.mark_agent_done("agent-abc123")
        post = buf.getvalue()
        delta = post[len(pre) :]
        assert "Done" in delta or "✓" in delta


class TestThreadSafety:
    def test_concurrent_log_message(self) -> None:
        disp, _ = _make_display()
        disp.register_agent("agent-abc123", "goal", None, 50)
        errors: list[Exception] = []

        def worker(i: int) -> None:
            try:
                for _ in range(20):
                    disp.log_message("agent-abc123", f"msg from thread {i}")
            except Exception as exc:
                errors.append(exc)

        threads = [threading.Thread(target=worker, args=(i,)) for i in range(4)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        assert errors == []


class TestGetConsole:
    def test_returns_constructor_console(self) -> None:
        buf = StringIO()
        console = Console(file=buf)
        disp = ReplDisplay(console)
        assert disp.get_console() is console


class TestPrintFinalOutput:
    def test_prints_rich_renderable(self) -> None:
        disp, buf = _make_display()
        disp.print_final_output(Text("final result"))
        output = buf.getvalue()
        assert "final result" in output


class TestLogResult:
    def test_success_result(self) -> None:
        disp, buf = _make_display()
        disp.register_agent("agent-abc123", "goal", None, 50)
        pre = buf.getvalue()
        disp.log_result("agent-abc123", "[green]OK[/green]", "found 5 items")
        post = buf.getvalue()
        delta = post[len(pre) :]
        assert "5 items" in delta

    def test_failure_result(self) -> None:
        disp, buf = _make_display()
        disp.register_agent("agent-abc123", "goal", None, 50)
        pre = buf.getvalue()
        disp.log_result("agent-abc123", "[red]FAIL[/red]", "timeout error")
        post = buf.getvalue()
        delta = post[len(pre) :]
        assert "timeout error" in delta
