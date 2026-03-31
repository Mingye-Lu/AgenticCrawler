from __future__ import annotations

import csv
import io
import json
from typing import Any

from rich.console import Console
from rich.syntax import Syntax

_default_console = Console()


def _get_console(console: Console | None) -> Console:
    """Return injected console or module-level fallback."""
    return console if console is not None else _default_console


def write_output(
    data: list[Any], fmt: str = "json", file_path: str | None = None, console: Console | None = None
) -> None:
    """Write extracted data in the requested format."""
    if fmt == "json":
        _write_json(data, file_path, console)
    elif fmt == "csv":
        _write_csv(data, file_path, console)
    else:
        _write_stdout(data, console)


def _write_json(data: list[Any], file_path: str | None, console: Console | None = None) -> None:
    con = _get_console(console)
    text = json.dumps(data, indent=2, ensure_ascii=False, default=str)
    if file_path:
        with open(file_path, "w", encoding="utf-8") as f:
            f.write(text)
        con.print(f"[green]Output written to {file_path}[/green]")
    else:
        con.print(Syntax(text, "json", theme="monokai"))


def _write_csv(data: list[Any], file_path: str | None, console: Console | None = None) -> None:
    con = _get_console(console)
    if not data:
        return

    # Flatten to list of dicts
    rows: list[dict[str, Any]] = []
    for item in data:
        if isinstance(item, dict):
            rows.append(item)
        elif isinstance(item, list):
            rows.extend(r for r in item if isinstance(r, dict))
        else:
            rows.append({"value": item})

    if not rows:
        con.print("[yellow]No tabular data to write as CSV[/yellow]")
        return

    # Collect all keys
    fieldnames: list[str] = []
    seen: set[str] = set()
    for row in rows:
        for key in row:
            if key not in seen:
                fieldnames.append(key)
                seen.add(key)

    output = io.StringIO() if not file_path else open(file_path, "w", encoding="utf-8", newline="")
    try:
        writer = csv.DictWriter(output, fieldnames=fieldnames, extrasaction="ignore")
        writer.writeheader()
        writer.writerows(rows)

        if not file_path:
            con.print(output.getvalue())
        else:
            con.print(f"[green]CSV written to {file_path}[/green]")
    finally:
        output.close()


def _write_stdout(data: list[Any], console: Console | None = None) -> None:
    con = _get_console(console)
    con.print(format_text(data))


def format_text(
    data: list[Any],
    summary: str | None = None,
    child_blocks: list[Any] | None = None,
) -> str:
    """Render extracted data as readable markdown text, not raw JSON.

    When *child_blocks* is provided each subagent's results are rendered in
    their own section rather than being merged into one flat list.
    """
    parts: list[str] = []

    if summary:
        parts.append(summary)
        parts.append("")

    # Render the root agent's own data (if any)
    if data:
        if child_blocks:
            parts.append("## Root agent")
            parts.append("")
        parts.append(_render_items(data))

    # Render each child block as its own section
    if child_blocks:
        for block in child_blocks:
            parts.append("")
            parts.append(f"## {block.child_id}: {block.sub_goal}")
            parts.append("")
            parts.append(_render_items(block.items))

    if not data and not child_blocks:
        return "No data extracted."

    return "\n".join(parts)


def _render_items(data: list[Any]) -> str:
    """Render a list of items as markdown (table, bullets, or mixed)."""
    parts: list[str] = []

    if all(isinstance(item, dict) for item in data):
        parts.append(_dicts_to_table(data))
    elif all(isinstance(item, (str, int, float)) for item in data):
        for item in data:
            parts.append(f"- {item}")
    else:
        for item in data:
            if isinstance(item, dict):
                parts.append(_dict_to_keyvalue(item))
                parts.append("")
            elif isinstance(item, list):
                parts.append(
                    _dicts_to_table(item) if item and isinstance(item[0], dict) else str(item)
                )
            else:
                parts.append(f"- {item}")

    return "\n".join(parts)


def _dicts_to_table(rows: list[dict[str, Any]]) -> str:
    """Render a list of dicts as a markdown table."""
    if not rows:
        return ""

    # Collect all keys in order
    keys: list[str] = []
    seen: set[str] = set()
    for row in rows:
        for k in row:
            if k not in seen:
                keys.append(k)
                seen.add(k)

    # Build table
    lines: list[str] = []
    header = "| " + " | ".join(keys) + " |"
    separator = "| " + " | ".join("---" for _ in keys) + " |"
    lines.append(header)
    lines.append(separator)

    for row in rows:
        cells = []
        for k in keys:
            val = row.get(k, "")
            cell = _flatten_value(val)
            cells.append(cell)
        lines.append("| " + " | ".join(cells) + " |")

    return "\n".join(lines)


def _dict_to_keyvalue(d: dict[str, Any]) -> str:
    """Render a single dict as key: value lines."""
    lines: list[str] = []
    for k, v in d.items():
        lines.append(f"**{k}**: {_flatten_value(v)}")
    return "\n".join(lines)


def _flatten_value(val: Any) -> str:
    """Flatten a value to a readable string, handling nested structures."""
    if isinstance(val, dict):
        # Render nested dict inline
        parts = [f"{k}: {_flatten_value(v)}" for k, v in val.items()]
        return ", ".join(parts)
    elif isinstance(val, list):
        if all(isinstance(item, dict) for item in val):
            # Nested list of dicts -> comma-separated summaries
            summaries = []
            for item in val:
                summary = ", ".join(f"{k}: {v}" for k, v in item.items())
                summaries.append(summary)
            return "; ".join(summaries)
        return ", ".join(str(item) for item in val)
    return str(val)
