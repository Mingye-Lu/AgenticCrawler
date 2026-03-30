import pytest

from agentic_crawler.actions.execute_js import ExecuteJsAction
from agentic_crawler.actions.go_back import GoBackAction
from agentic_crawler.actions.hover import HoverAction
from agentic_crawler.actions.press_key import PressKeyAction
from agentic_crawler.actions.select_option import SelectOptionAction
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
    assert len(schemas) == 14  # 7 original + 6 new + done


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
