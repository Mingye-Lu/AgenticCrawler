import pytest

from agentic_crawler.actions.execute_js import ExecuteJsAction
from agentic_crawler.actions.go_back import GoBackAction
from agentic_crawler.actions.hover import HoverAction
from agentic_crawler.actions.press_key import PressKeyAction
from agentic_crawler.actions.select_option import SelectOptionAction
from agentic_crawler.actions.save_file import SaveFileAction
from agentic_crawler.actions.switch_tab import SwitchTabAction
from agentic_crawler.agent.tools import get_action_registry, get_tool_schemas


def test_new_actions_in_registry() -> None:
    registry = get_action_registry()
    expected = {"select_option", "go_back", "execute_js", "hover", "press_key", "switch_tab"}
    for name in expected:
        assert name in registry, f"Missing action: {name}"


def test_new_tool_schemas_present() -> None:
    schemas = get_tool_schemas()
    names = {s["name"] for s in schemas}
    expected = {"select_option", "go_back", "execute_js", "hover", "press_key", "switch_tab"}
    for name in expected:
        assert name in names, f"Missing schema: {name}"


def test_total_tool_count() -> None:
    schemas = get_tool_schemas()
    assert (
        len(schemas) == 18
    )  # 7 original + 6 new + done + fork + wait_for_subagents + list_resources + save_file


@pytest.mark.asyncio
async def test_select_option_requires_selector() -> None:
    action = SelectOptionAction()
    result = await action.execute(router=None, params={})  # type: ignore[arg-type]
    assert not result.success
    assert "selector" in result.observation.lower()


@pytest.mark.asyncio
async def test_select_option_requires_value_label_or_index() -> None:
    action = SelectOptionAction()
    result = await action.execute(
        router=None,  # type: ignore[arg-type]
        params={"selector": "select#country"},
    )
    assert not result.success
    assert "value" in result.observation.lower() or "label" in result.observation.lower()


@pytest.mark.asyncio
async def test_execute_js_requires_script() -> None:
    action = ExecuteJsAction()
    result = await action.execute(router=None, params={})  # type: ignore[arg-type]
    assert not result.success
    assert "script" in result.observation.lower()


@pytest.mark.asyncio
async def test_hover_requires_selector() -> None:
    action = HoverAction()
    result = await action.execute(router=None, params={})  # type: ignore[arg-type]
    assert not result.success
    assert "selector" in result.observation.lower()


@pytest.mark.asyncio
async def test_press_key_requires_key() -> None:
    action = PressKeyAction()
    result = await action.execute(router=None, params={})  # type: ignore[arg-type]
    assert not result.success
    assert "key" in result.observation.lower()


def test_schema_required_fields() -> None:
    schemas = get_tool_schemas()
    by_name = {s["name"]: s for s in schemas}

    assert "selector" in by_name["select_option"]["parameters"]["required"]
    assert "script" in by_name["execute_js"]["parameters"]["required"]
    assert "selector" in by_name["hover"]["parameters"]["required"]
    assert "key" in by_name["press_key"]["parameters"]["required"]
    assert by_name["go_back"]["parameters"]["properties"] == {}
    assert "url" in by_name["save_file"]["parameters"]["required"]


def test_save_file_in_registry() -> None:
    registry = get_action_registry()
    assert "save_file" in registry


def test_save_file_schema_present() -> None:
    schemas = get_tool_schemas()
    names = {s["name"] for s in schemas}
    assert "save_file" in names


@pytest.mark.asyncio
async def test_save_file_requires_url() -> None:
    action = SaveFileAction()
    result = await action.execute(router=None, params={})  # type: ignore[arg-type]
    assert not result.success
    assert "url" in result.observation.lower()


@pytest.mark.asyncio
async def test_save_file_path_traversal(tmp_path: object) -> None:
    from pathlib import Path
    from unittest.mock import AsyncMock, MagicMock

    workspace = Path(str(tmp_path)) / "ws"
    workspace.mkdir()

    router = MagicMock()
    router.workspace_dir = workspace

    action = SaveFileAction()
    result = await action.execute(router, params={"url": "http://x.com/f.txt", "subdir": "../../etc"})
    assert not result.success
    assert "traversal" in result.observation.lower()


@pytest.mark.asyncio
async def test_save_file_downloads_and_writes(tmp_path: object) -> None:
    from pathlib import Path
    from unittest.mock import AsyncMock, MagicMock

    workspace = Path(str(tmp_path)) / "ws"
    workspace.mkdir()

    mock_response = MagicMock()
    mock_response.content = b"hello world"
    mock_response.raise_for_status = MagicMock()

    router = MagicMock()
    router.workspace_dir = workspace
    router.http.client.get = AsyncMock(return_value=mock_response)

    action = SaveFileAction()
    result = await action.execute(router, params={"url": "http://example.com/data/report.pdf"})
    assert result.success
    saved = workspace / "report.pdf"
    assert saved.exists()
    assert saved.read_bytes() == b"hello world"
    assert "11 bytes" in result.observation
