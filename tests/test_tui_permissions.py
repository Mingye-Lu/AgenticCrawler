"""Tests for tui/permissions.py — 3-tier permission model."""

from __future__ import annotations

import pytest

from agentic_crawler.tui.permissions import PermissionMode, PermissionPolicy


ALL_15_TOOLS = [
    "navigate",
    "click",
    "fill_form",
    "scroll",
    "extract_data",
    "screenshot",
    "wait",
    "select_option",
    "go_back",
    "execute_js",
    "hover",
    "press_key",
    "switch_tab",
    "list_resources",
    "save_file",
]


def test_default_mode_is_full_access():
    policy = PermissionPolicy()
    assert policy.mode == PermissionMode.FullAccess


def test_full_access_authorizes_all_tools():
    policy = PermissionPolicy(PermissionMode.FullAccess)
    for tool in ALL_15_TOOLS:
        assert policy.authorize(tool), f"FullAccess should allow '{tool}'"


def test_readonly_allows_read_tools():
    policy = PermissionPolicy(PermissionMode.ReadOnly)
    assert policy.authorize("extract_data")
    assert policy.authorize("screenshot")
    assert policy.authorize("scroll")
    assert policy.authorize("go_back")
    assert policy.authorize("wait")
    assert policy.authorize("hover")
    assert policy.authorize("switch_tab")
    assert policy.authorize("list_resources")


def test_readonly_blocks_full_access_tools():
    policy = PermissionPolicy(PermissionMode.ReadOnly)
    assert not policy.authorize("navigate")
    assert not policy.authorize("click")
    assert not policy.authorize("fill_form")
    assert not policy.authorize("execute_js")


def test_workspace_write_allows_save_file():
    policy = PermissionPolicy(PermissionMode.WorkspaceWrite)
    assert policy.authorize("save_file")


def test_workspace_write_blocks_full_access_tools():
    policy = PermissionPolicy(PermissionMode.WorkspaceWrite)
    assert not policy.authorize("execute_js")
    assert not policy.authorize("navigate")
    assert not policy.authorize("click")


def test_all_15_tools_in_requirements():
    assert set(ALL_15_TOOLS) == set(PermissionPolicy.TOOL_REQUIREMENTS.keys())


def test_unknown_tool_defaults_to_full_access_requirement():
    """Unknown tools are treated as FullAccess-required — blocked by ReadOnly."""
    policy_readonly = PermissionPolicy(PermissionMode.ReadOnly)
    assert not policy_readonly.authorize("nonexistent_tool")

    policy_full = PermissionPolicy(PermissionMode.FullAccess)
    assert policy_full.authorize("nonexistent_tool")
