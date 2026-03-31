"""
Tests for the agent's final text output rendering.
The final agent response should be parsed markdown text, not free-form JSON.
"""
from __future__ import annotations

from io import StringIO
from typing import Any

import pytest

from agentic_crawler.output.writer import format_text, write_output


class TestFormatText:
    """format_text should produce readable markdown from extracted data."""

    def test_list_of_dicts_renders_as_table(self) -> None:
        data = [
            {"name": "Widget A", "price": "$10.00"},
            {"name": "Widget B", "price": "$20.00"},
        ]
        text = format_text(data)
        # Should contain a markdown table header
        assert "| name" in text or "| Name" in text
        assert "Widget A" in text
        assert "Widget B" in text
        assert "$10.00" in text
        # Should NOT be raw JSON
        assert text.strip()[0] != "["

    def test_single_dict_renders_as_key_value(self) -> None:
        data = [{"title": "About Us", "url": "https://example.com/about"}]
        text = format_text(data)
        assert "title" in text.lower() or "Title" in text
        assert "About Us" in text
        assert "https://example.com/about" in text
        assert text.strip()[0] != "["

    def test_nested_data_renders_readable(self) -> None:
        data = [{"products": [{"name": "A"}, {"name": "B"}]}]
        text = format_text(data)
        assert "A" in text
        assert "B" in text
        assert text.strip()[0] != "["

    def test_scalar_items_render_as_list(self) -> None:
        data = ["https://example.com/a", "https://example.com/b"]
        text = format_text(data)
        assert "https://example.com/a" in text
        assert "https://example.com/b" in text
        # Should look like a bullet list or numbered list
        assert "-" in text or "1." in text

    def test_empty_data(self) -> None:
        text = format_text([])
        assert "no data" in text.lower() or text.strip() == ""

    def test_mixed_types(self) -> None:
        data = [{"name": "Widget"}, "plain string", 42]
        text = format_text(data)
        assert "Widget" in text
        assert "plain string" in text
        assert "42" in text

    def test_summary_included_when_provided(self) -> None:
        data = [{"name": "Widget A"}]
        text = format_text(data, summary="Found 1 product on the page.")
        assert "Found 1 product on the page." in text

    def test_output_is_not_json(self) -> None:
        """The output must never be a raw JSON array or object."""
        data = [{"a": 1, "b": 2}]
        text = format_text(data)
        stripped = text.strip()
        # Must not start with [ or {
        assert stripped[0] not in ("[", "{"), f"Output looks like raw JSON: {stripped[:80]}"


class TestWriteOutputStdout:
    """write_output with format='stdout' should produce text, not JSON."""

    def test_stdout_format_not_json(self, capsys: pytest.CaptureFixture[str]) -> None:
        data = [{"name": "Widget A", "price": "$10.00"}]
        write_output(data, fmt="stdout")
        captured = capsys.readouterr().out
        assert "Widget A" in captured
        # Should not be a raw JSON dump
        assert captured.strip()[0] not in ("[", "{")
