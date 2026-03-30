from __future__ import annotations

import csv
import io
import json
import sys
from typing import Any

from rich.console import Console
from rich.syntax import Syntax

console = Console()


def write_output(data: list[Any], fmt: str = "json", file_path: str | None = None) -> None:
    """Write extracted data in the requested format."""
    if fmt == "json":
        _write_json(data, file_path)
    elif fmt == "csv":
        _write_csv(data, file_path)
    else:
        _write_stdout(data)


def _write_json(data: list[Any], file_path: str | None) -> None:
    text = json.dumps(data, indent=2, ensure_ascii=False, default=str)
    if file_path:
        with open(file_path, "w", encoding="utf-8") as f:
            f.write(text)
        console.print(f"[green]Output written to {file_path}[/green]")
    else:
        console.print(Syntax(text, "json", theme="monokai"))


def _write_csv(data: list[Any], file_path: str | None) -> None:
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
        console.print("[yellow]No tabular data to write as CSV[/yellow]")
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
            console.print(output.getvalue())
        else:
            console.print(f"[green]CSV written to {file_path}[/green]")
    finally:
        output.close()


def _write_stdout(data: list[Any]) -> None:
    for item in data:
        if isinstance(item, (dict, list)):
            console.print_json(json.dumps(item, default=str))
        else:
            console.print(str(item))
