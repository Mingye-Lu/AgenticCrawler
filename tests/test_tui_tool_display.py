import pytest
from agentic_crawler.tui.tool_display import (
    format_tool_start,
    format_tool_result,
    format_tool_call,
)


class TestFormatToolStart:
    def test_contains_box_drawing_chars(self):
        result = format_tool_start("navigate", {"url": "https://example.com"})
        assert "╭" in result
        assert "╰" in result

    def test_contains_tool_name(self):
        result = format_tool_start("navigate", {"url": "https://example.com"})
        assert "navigate" in result

    def test_contains_param_key_and_value(self):
        result = format_tool_start("click", {"selector": "#btn", "timeout": 5})
        assert "selector" in result
        assert "#btn" in result

    def test_empty_params(self):
        result = format_tool_start("screenshot", {})
        assert "╭" in result
        assert "screenshot" in result

    def test_contains_side_borders(self):
        result = format_tool_start("navigate", {"url": "https://example.com"})
        assert "│" in result


class TestFormatToolResult:
    def test_success_contains_checkmark(self):
        result = format_tool_result("navigate", success=True, observation="Page loaded")
        assert "✓" in result

    def test_failure_contains_cross(self):
        result = format_tool_result("navigate", success=False, observation="Timeout")
        assert "✗" in result

    def test_contains_observation(self):
        result = format_tool_result(
            "navigate", success=True, observation="Page loaded successfully"
        )
        assert "Page loaded successfully" in result

    def test_truncates_long_observation_at_max_lines(self):
        long_obs = "\n".join(f"line {i}" for i in range(50))
        result = format_tool_result("navigate", success=True, observation=long_obs, max_lines=5)
        assert "more lines" in result
        assert "line 5" not in result

    def test_no_truncation_when_within_max_lines(self):
        short_obs = "\n".join(f"line {i}" for i in range(3))
        result = format_tool_result("navigate", success=True, observation=short_obs, max_lines=20)
        assert "more lines" not in result
        assert "line 2" in result

    def test_none_observation(self):
        result = format_tool_result("navigate", success=True, observation=None)
        assert "✓" in result

    def test_contains_box_drawing_chars(self):
        result = format_tool_result("navigate", success=True, observation="ok")
        assert "╭" in result
        assert "╰" in result


class TestFormatToolCall:
    def test_contains_tool_name(self):
        result = format_tool_call("navigate", {"url": "https://example.com"}, True, "Loaded")
        assert "navigate" in result

    def test_success_contains_checkmark(self):
        result = format_tool_call("navigate", {"url": "https://example.com"}, True, "Loaded")
        assert "✓" in result

    def test_failure_contains_cross(self):
        result = format_tool_call("navigate", {"url": "https://example.com"}, False, "Error")
        assert "✗" in result

    def test_contains_both_start_and_result(self):
        result = format_tool_call("click", {"selector": "#btn"}, True, "Clicked")
        assert "selector" in result
        assert "Clicked" in result

    def test_is_string(self):
        result = format_tool_call("navigate", {}, True, None)
        assert isinstance(result, str)
