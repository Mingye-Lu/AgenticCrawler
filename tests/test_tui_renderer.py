from __future__ import annotations

from agentic_crawler.tui.renderer import MarkdownStreamState


class TestPushReturnsNoneForPartialInput:
    def test_single_word(self) -> None:
        s = MarkdownStreamState()
        assert s.push("hello") is None

    def test_single_line_no_trailing_blank(self) -> None:
        s = MarkdownStreamState()
        assert s.push("# Header\n") is None


class TestPushRendersAtEmptyLine:
    def test_paragraph_break(self) -> None:
        s = MarkdownStreamState()
        result = s.push("Hello world\n\n")
        assert result is not None
        assert "Hello world" in result

    def test_accumulated_then_boundary(self) -> None:
        s = MarkdownStreamState()
        assert s.push("first line\n") is None
        result = s.push("\n")
        assert result is not None
        assert "first line" in result


class TestCodeBlockDetectionBlocksBoundary:
    def test_empty_line_inside_code_block(self) -> None:
        s = MarkdownStreamState()
        assert s.push("```python\n") is None
        assert s.push("x = 1\n") is None
        # empty line INSIDE fence must NOT be treated as safe boundary
        assert s.push("\n") is None
        assert s.push("y = 2\n") is None


class TestCodeBlockCloseEnablesBoundary:
    def test_boundary_after_code_block(self) -> None:
        s = MarkdownStreamState()
        assert s.push("```\ncode\n```\n") is not None

    def test_empty_line_after_closed_block(self) -> None:
        s = MarkdownStreamState()
        assert s.push("```\n") is None
        assert s.push("code here\n") is None
        # closing fence line marks boundary even without extra blank line
        assert s.push("```\n") is not None


class TestFlushRendersRemainingBuffer:
    def test_flush_partial_content(self) -> None:
        s = MarkdownStreamState()
        s.push("Some partial text")
        result = s.flush()
        assert result is not None
        assert "Some partial text" in result

    def test_flush_after_push_rendered(self) -> None:
        s = MarkdownStreamState()
        s.push("Paragraph one\n\nParagraph two")
        result = s.flush()
        assert result is not None
        assert "Paragraph two" in result


class TestFlushReturnsNoneWhenEmpty:
    def test_flush_empty(self) -> None:
        s = MarkdownStreamState()
        assert s.flush() is None

    def test_flush_after_full_render(self) -> None:
        s = MarkdownStreamState()
        s.push("text\n\n")
        assert s.flush() is None


class TestMultipleSequentialPushes:
    def test_three_pushes_then_boundary(self) -> None:
        s = MarkdownStreamState()
        assert s.push("a") is None
        assert s.push("b") is None
        assert s.push("c\n\n") is not None

    def test_accumulation_preserves_content(self) -> None:
        s = MarkdownStreamState()
        s.push("Hello ")
        s.push("World")
        result = s.flush()
        assert result is not None
        assert "Hello" in result
        assert "World" in result


class TestFullMarkdownRendering:
    def test_header_and_code_block(self) -> None:
        s = MarkdownStreamState()
        result = s.push("# Title\n\n")
        assert result is not None
        assert "Title" in result

    def test_complex_document(self) -> None:
        s = MarkdownStreamState()
        r1 = s.push("# Hello\n\n")
        assert r1 is not None

        assert s.push("```python\n") is None
        assert s.push("print('hi')\n") is None
        # empty line in code block must NOT trigger boundary
        assert s.push("\n") is None
        r2 = s.push("```\n")
        assert r2 is not None

        s.push("Done!")
        r3 = s.flush()
        assert r3 is not None
        assert "Done" in r3

    def test_tilde_fence(self) -> None:
        s = MarkdownStreamState()
        assert s.push("~~~\n") is None
        assert s.push("code\n") is None
        # ~~~ inside fence — empty line must not trigger boundary
        assert s.push("\n") is None
        result = s.push("~~~\n")
        assert result is not None

    def test_rendered_output_is_string(self) -> None:
        s = MarkdownStreamState()
        result = s.push("**bold text**\n\n")
        assert isinstance(result, str)
        assert len(result) > 0
