"""3-tier permission model for crawler tools."""

from __future__ import annotations

from enum import IntEnum


class PermissionMode(IntEnum):
    ReadOnly = 1
    WorkspaceWrite = 2
    FullAccess = 3


class PermissionPolicy:
    TOOL_REQUIREMENTS: dict[str, PermissionMode] = {
        # ReadOnly tools — observe only, no state change
        "extract_data": PermissionMode.ReadOnly,
        "screenshot": PermissionMode.ReadOnly,
        "list_resources": PermissionMode.ReadOnly,
        "scroll": PermissionMode.ReadOnly,
        "go_back": PermissionMode.ReadOnly,
        "wait": PermissionMode.ReadOnly,
        "hover": PermissionMode.ReadOnly,
        "switch_tab": PermissionMode.ReadOnly,
        # WorkspaceWrite tools — write to local disk only
        "save_file": PermissionMode.WorkspaceWrite,
        # FullAccess tools — mutate remote state / execute code
        "navigate": PermissionMode.FullAccess,
        "click": PermissionMode.FullAccess,
        "fill_form": PermissionMode.FullAccess,
        "execute_js": PermissionMode.FullAccess,
        "select_option": PermissionMode.FullAccess,
        "press_key": PermissionMode.FullAccess,
    }

    def __init__(self, mode: PermissionMode = PermissionMode.FullAccess) -> None:
        self.mode = mode

    def authorize(self, tool_name: str) -> bool:
        """Return True if the current mode allows the given tool."""
        requirement = self.TOOL_REQUIREMENTS.get(tool_name, PermissionMode.FullAccess)
        return self.mode >= requirement
