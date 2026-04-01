from __future__ import annotations

_CYAN = "\033[36m"
_GREEN = "\033[32m"
_RED = "\033[31m"
_DIM = "\033[2m"
_RESET = "\033[0m"

_BOX_WIDTH = 46

_TL = "\u256d"
_TR = "\u256e"
_BL = "\u2570"
_BR = "\u256f"
_H = "\u2500"
_V = "\u2502"

_CHECK = "\u2713"
_CROSS = "\u2717"


def _header(label: str) -> str:
    inner = f" {label} "
    dashes = _H * (_BOX_WIDTH - len(inner))
    return f"{_TL}{_H}{inner}{dashes}{_TR}"


def _footer() -> str:
    return f"{_BL}{_H * (_BOX_WIDTH + 1)}{_BR}"


def _body_line(text: str) -> str:
    return f"{_V}  {text}"


def format_tool_start(tool_name: str, params: dict) -> str:
    label = f"{_CYAN}{tool_name}{_RESET}"
    lines = [_header(label)]
    for key, val in params.items():
        lines.append(_body_line(f"{_DIM}{key}: {val}{_RESET}"))
    lines.append(_footer())
    return "\n".join(lines)


def format_tool_result(
    tool_name: str,
    success: bool,
    observation: str | None,
    max_lines: int = 20,
) -> str:
    icon, color = (_CHECK, _GREEN) if success else (_CROSS, _RED)
    label = f"{_CYAN}{tool_name}{_RESET} {color}{icon}{_RESET}"
    lines = [_header(label)]
    if observation is not None:
        obs_lines = observation.split("\n")
        truncated = obs_lines[:max_lines]
        remainder = len(obs_lines) - max_lines
        for ln in truncated:
            lines.append(_body_line(ln))
        if remainder > 0:
            lines.append(_body_line(f"... ({remainder} more lines)"))
    lines.append(_footer())
    return "\n".join(lines)


def format_tool_call(
    tool_name: str,
    params: dict,
    success: bool,
    observation: str | None,
) -> str:
    return (
        format_tool_start(tool_name, params)
        + "\n"
        + format_tool_result(tool_name, success, observation)
    )
