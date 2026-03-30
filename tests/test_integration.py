"""Integration tests that run the full agent loop against real websites with a real LLM API.

These tests require:
- OAuth tokens stored at ~/.codex/auth.json (run `agentic-crawler login`)
- Network access

Run with:  pytest tests/test_integration.py -v -s --timeout=120
Skip in CI: these are marked with @pytest.mark.integration
"""

from __future__ import annotations

import pytest

from agentic_crawler.agent.loop import run_agent
from agentic_crawler.config import Settings


pytestmark = pytest.mark.integration


def _make_settings(**overrides: object) -> Settings:
    defaults: dict[str, object] = {
        "llm_provider": "openai",
        "openai_auth_method": "oauth",
        "headless": True,
        "max_steps": 15,
        "temperature": 0.0,
    }
    defaults.update(overrides)
    return Settings(**defaults)  # type: ignore[arg-type]


@pytest.mark.asyncio
async def test_extract_books_from_toscrape() -> None:
    settings = _make_settings(max_steps=10)
    from agentic_crawler.agent.state import AgentState
    from agentic_crawler.agent.tools import get_action_registry, get_tool_schemas
    from agentic_crawler.agent.prompt_builder import build_messages
    from agentic_crawler.fetcher.router import FetcherRouter
    from agentic_crawler.llm.registry import get_provider

    provider = get_provider(settings)
    router = FetcherRouter(headless=True, browser_timeout=settings.browser_timeout)
    actions = get_action_registry()
    tool_schemas = get_tool_schemas()
    state = AgentState(
        goal="Navigate to https://books.toscrape.com and extract the title and price of the first 3 books on the homepage.",
        max_steps=10,
    )

    try:
        messages = build_messages(state, provider=settings.llm_provider)
        response = await provider.complete(
            messages=messages,
            tools=tool_schemas,
            temperature=0.0,
        )

        assert response.has_tool_calls or response.text, "LLM returned empty response"

        if response.has_tool_calls:
            first_call = response.tool_calls[0]
            assert first_call.name in actions or first_call.name == "done", (
                f"LLM called unknown tool: {first_call.name}"
            )
            assert first_call.name == "navigate", (
                f"Expected first action to be navigate, got {first_call.name}"
            )
            url = first_call.arguments.get("url", "")
            assert "books.toscrape" in url, f"Expected books.toscrape URL, got {url}"
    finally:
        await router.close()
        await provider.close()


@pytest.mark.asyncio
async def test_full_agent_loop_small() -> None:
    settings = _make_settings(max_steps=8, output_format="json")

    from agentic_crawler.agent.state import AgentState
    from agentic_crawler.agent.tools import get_action_registry, get_tool_schemas
    from agentic_crawler.agent.prompt_builder import build_messages, build_plan_messages
    from agentic_crawler.fetcher.router import FetcherRouter
    from agentic_crawler.llm.registry import get_provider
    from agentic_crawler.parser.html_parser import page_content_to_text, parse_html

    provider = get_provider(settings)
    router = FetcherRouter(headless=True, browser_timeout=settings.browser_timeout)
    actions = get_action_registry()
    tool_schemas = get_tool_schemas()

    state = AgentState(
        goal="Go to https://books.toscrape.com and extract the title and price of the first 2 books.",
        max_steps=8,
    )

    try:
        plan_msgs = build_plan_messages(state.goal)
        plan_resp = await provider.complete(messages=plan_msgs, temperature=0.0)
        assert plan_resp.text, "Plan response was empty"
        state.plan = [line.strip() for line in plan_resp.text.strip().splitlines() if line.strip()]

        steps_executed = 0
        max_loop = 8
        while not state.done and steps_executed < max_loop:
            messages = build_messages(state, provider=settings.llm_provider)
            response = await provider.complete(
                messages=messages,
                tools=tool_schemas,
                temperature=0.0,
            )
            state.total_tokens += sum(response.usage.values())

            if response.has_tool_calls:
                for tc in response.tool_calls:
                    if tc.name == "done":
                        state.mark_done(tc.arguments.get("summary", "done"))
                        break
                    action = actions.get(tc.name)
                    if action:
                        result = await action.execute(router, tc.arguments)
                        state.add_step(
                            tc.name,
                            tc.arguments,
                            result.observation,
                            result.success,
                            tool_call_id=tc.id,
                        )
                        if result.new_url:
                            state.current_url = result.new_url
                        if result.new_html:
                            state.current_html = result.new_html
                        if result.data is not None:
                            state.extracted_data.append(result.data)
                    else:
                        state.add_step(
                            tc.name, tc.arguments, f"Unknown: {tc.name}", False, tool_call_id=tc.id
                        )
                    steps_executed += 1
                    if state.done:
                        break
            else:
                steps_executed += 1

            if state.current_html and state.current_url:
                content = parse_html(state.current_html, state.current_url)
                state.page_summary = page_content_to_text(content)

        assert state.step_count > 0, "Agent took no steps"
        assert state.total_tokens > 0, "No tokens used — LLM not called?"
        assert any("books.toscrape" in (s.params.get("url", "") or "") for s in state.history), (
            "Agent never navigated to books.toscrape.com"
        )
    finally:
        await router.close()
        await provider.close()


@pytest.mark.asyncio
async def test_tool_schemas_accepted_by_api() -> None:
    settings = _make_settings(max_steps=1)
    from agentic_crawler.agent.tools import get_tool_schemas
    from agentic_crawler.llm.registry import get_provider

    provider = get_provider(settings)
    tool_schemas = get_tool_schemas()

    try:
        response = await provider.complete(
            messages=[
                {"role": "system", "content": "You are a test agent."},
                {"role": "user", "content": "Navigate to https://example.com"},
            ],
            tools=tool_schemas,
            temperature=0.0,
        )
        assert response.has_tool_calls or response.text, "API returned nothing"
    finally:
        await provider.close()
