import pytest

from agentic_crawler.actions.extract import ExtractDataAction
from agentic_crawler.actions.navigate import NavigateAction


@pytest.mark.asyncio
async def test_extract_data_action() -> None:
    action = ExtractDataAction()
    result = await action.execute(
        router=None,  # type: ignore[arg-type]  # extract doesn't use router
        params={"instruction": "Extract prices", "data": [{"name": "Widget", "price": 10}]},
    )
    assert result.success
    assert result.data == [{"name": "Widget", "price": 10}]


@pytest.mark.asyncio
async def test_extract_data_action_no_params() -> None:
    action = ExtractDataAction()
    result = await action.execute(
        router=None,  # type: ignore[arg-type]
        params={},
    )
    assert not result.success
