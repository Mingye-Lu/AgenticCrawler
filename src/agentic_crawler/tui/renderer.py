from __future__ import annotations

import re
from io import StringIO

from rich.console import Console
from rich.markdown import Markdown


def _split_lines_inclusive(text: str) -> list[str]:
    """Split text on newlines, keeping the delimiter attached (like Rust split_inclusive)."""
    return [part for part in re.split(r"(?<=\n)", text) if part]


def _find_stream_safe_boundary(markdown: str) -> int | None:
    # Safe boundary: blank line or closing fence line, outside code fences.
    # Mirrors claw-code render.rs find_stream_safe_boundary().
    in_fence = False
    last_boundary: int | None = None
    cursor = 0

    for line in _split_lines_inclusive(markdown):
        trimmed = line.lstrip()
        line_end = cursor + len(line)

        if trimmed.startswith("```") or trimmed.startswith("~~~"):
            in_fence = not in_fence
            if not in_fence:
                last_boundary = line_end
            cursor = line_end
            continue

        if in_fence:
            cursor = line_end
            continue

        if not trimmed or trimmed == "\n":
            last_boundary = line_end

        cursor = line_end

    return last_boundary


def _render_markdown(text: str) -> str:
    buf = StringIO()
    console = Console(file=buf, highlight=False, markup=False)
    console.print(Markdown(text))
    return buf.getvalue()


class MarkdownStreamState:
    __slots__ = ("_buffer",)

    def __init__(self) -> None:
        self._buffer = ""

    def push(self, delta: str) -> str | None:
        self._buffer += delta

        boundary = _find_stream_safe_boundary(self._buffer)
        if boundary is None:
            return None

        ready = self._buffer[:boundary]
        self._buffer = self._buffer[boundary:]
        return _render_markdown(ready)

    def flush(self) -> str | None:
        if not self._buffer.strip():
            self._buffer = ""
            return None

        text = self._buffer
        self._buffer = ""
        return _render_markdown(text)
