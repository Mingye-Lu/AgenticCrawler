from __future__ import annotations

import json
from typing import Any

from agentic_crawler.agent.state import AgentState, StepRecord

_SEARCH_CONSTRAINTS_BASE = """\
- Navigate directly to full URLs rather than filling search forms. \
For example, use navigate(url="https://www.bing.com/search?q=my+query") instead of filling a search box.
- Simplify search queries: remove underscores, special punctuation, and filename-style formatting. Use clean, natural keywords.
- If a search returns no results, try progressively simpler queries: drop subtitles, use fewer keywords, try alternate phrasings.
- Try multiple search engines if one fails — both Bing and DuckDuckGo are available.
- Do NOT use filetype: operators on DuckDuckGo — include the file type as a keyword instead (e.g., "pdf" or "epub")."""

_GOOGLE_HEADLESS_WARNING = (
    "- Google Search will likely fail in headless mode. "
    "Prefer Bing (bing.com/search?q=...) or DuckDuckGo (duckduckgo.com/?q=...) instead."
)


def _search_constraints(headless: bool = True) -> str:
    if headless:
        return _GOOGLE_HEADLESS_WARNING + "\n" + _SEARCH_CONSTRAINTS_BASE
    return _SEARCH_CONSTRAINTS_BASE


_SYSTEM_PROMPT_TEMPLATE = """\
<role>
You are an autonomous web crawling agent. You navigate websites, interact with pages, \
and extract structured data to accomplish a user's goal.
</role>

<instructions>
- Think step by step about how to achieve the goal.
- Use the tools provided to interact with web pages.
- When navigating, always use full URLs (including https://).
- When extracting data, provide it in structured JSON format via the extract_data tool.
- If a page requires JavaScript, interactive elements will be available through click, fill_form, etc.
- When you have accomplished the goal, call the 'done' tool with a summary.
</instructions>

<constraints>
- Do NOT loop indefinitely. If you cannot make progress after several attempts, call 'done' and explain what you found.
- Keep extracted data clean and well-structured.
- Do NOT output markdown tables with more than 5 columns. If more fields are needed, split them into \
multiple tables or use a vertical key-value format instead.
</constraints>

<error-recovery>
When you encounter errors, follow this escalation ladder:
1. Retry with a different CSS selector or XPath.
2. Try a different URL or page on the same site.
3. Try a different search engine or search query.
4. Call 'done' with whatever partial results you have and explain the blocker.
</error-recovery>

<search-strategy>
{search_constraints}
</search-strategy>

<navigation-strategy>
- After navigating via a search engine, extract the URLs you need from the page content and navigate directly to those URLs.
- Only use click when you need to interact with a specific page element (buttons, pagination, tabs). \
For following links, prefer navigate with the link's href URL.
- The page content shown to you already contains links with their URLs. Use navigate(url=...) to follow them.
- Use go_back to return to the previous page instead of re-navigating when you need to backtrack.
</navigation-strategy>

<interaction-tools>
- Use hover to reveal dropdown menus, tooltips, or mega-menus before clicking items inside them.
- Use select_option for <select> dropdowns (do NOT click individual options — use select_option with value, label, or index).
- Use press_key for keyboard actions: Enter to submit, Escape to close modals, Tab to move between fields, ArrowDown to navigate dropdown lists.
- Use execute_js when you need computed values, want to trigger page events, or need to reach data that CSS selectors cannot express.
- Use switch_tab when a click opens content in a new browser tab — switch to it and then continue working.
</interaction-tools>

<context>
Each turn, you will see:
- The current page content (title, text, links, forms)
- Your action history
- The original goal

Choose your next action wisely to make progress toward the goal.
</context>

<parallel-exploration>
- Use the `fork` tool to spawn a subagent on a separate browser tab when you need to explore multiple pages simultaneously.
- Each subagent gets a copy of your history and works independently.
- You can fork multiple subagents at once (up to the configured limit).
- After forking, you continue working — subagents run in parallel.
- Use `wait_for_subagents` to pause and collect results from all active subagents.
- When you call `done`, the system automatically waits for any active subagents and merges their data.
- Subagents can also fork their own subagents (up to the configured depth limit).
</parallel-exploration>
"""

HISTORY_WINDOW = 15  # Max recent steps to include
HISTORY_PIN = 2  # Always keep the first N steps visible


def _windowed_history(history: list[StepRecord]) -> list[StepRecord]:
    """Return a history window that always pins the first HISTORY_PIN steps."""
    if len(history) <= HISTORY_WINDOW:
        return list(history)
    pinned = history[:HISTORY_PIN]
    tail_size = HISTORY_WINDOW - HISTORY_PIN
    recent = history[-tail_size:]
    return pinned + recent


def build_messages(
    state: AgentState,
    provider: str = "claude",
    active_children: list[dict[str, str]] | None = None,
    headless: bool = True,
) -> list[dict[str, Any]]:
    """Build the message list for the LLM from current agent state."""
    system_prompt = _SYSTEM_PROMPT_TEMPLATE.format(
        search_constraints=_search_constraints(headless),
    )
    messages: list[dict[str, Any]] = [
        {"role": "system", "content": system_prompt},
    ]

    # Initial user message with the goal
    user_content = f"## Goal\n{state.goal}\n"

    if state.plan:
        plan_lines: list[str] = []
        for i, step in enumerate(state.plan):
            marker = "[x]" if i < state.step_count else "[ ]"
            plan_lines.append(f"{i + 1}. {marker} {step}")
        user_content += "\n## Plan\n" + "\n".join(plan_lines)

    messages.append({"role": "user", "content": user_content})

    recent_history = _windowed_history(state.history)
    is_claude = provider == "claude"

    if is_claude:
        _append_history_claude(messages, recent_history)
    else:
        _append_history_openai(messages, recent_history)

    # Current page context
    if state.page_summary:
        page_msg = f"## Current Page\n{state.page_summary}"
        if state.extracted_data:
            page_msg += (
                f"\n\n## Data Extracted So Far\n{len(state.extracted_data)} item(s) collected."
            )
        if active_children:
            page_msg += f"\n\n## Active Subagents ({len(active_children)})\n"
            for child in active_children:
                page_msg += f"- [{child['id']}] working on: {child['sub_goal']}\n"
        page_msg += "\n\nWhat is your next action?"

        if is_claude:
            _append_user_content_claude(messages, page_msg)
        else:
            messages.append({"role": "user", "content": page_msg})
    elif not state.history:
        prompt = "You have not visited any page yet. Start by navigating to a relevant URL."
        if is_claude:
            _append_user_content_claude(messages, prompt)
        else:
            messages.append({"role": "user", "content": prompt})

    return messages


def build_plan_messages(goal: str, headless: bool = True) -> list[dict[str, Any]]:
    constraints = _search_constraints(headless)
    return [
        {
            "role": "system",
            "content": (
                "You are a planning agent. Given a web crawling goal, produce a concise step-by-step plan. "
                "Each step should be a short action description. Return ONLY the plan as a numbered list, nothing else.\n\n"
                "<constraints>\n"
                f"{constraints}\n"
                "- Prefer navigate(url=...) over click for following links.\n"
                "</constraints>"
            ),
        },
        {"role": "user", "content": f"Goal: {goal}\n\nProduce a step-by-step plan:"},
    ]


_TEXT_ONLY_NUDGE = (
    "You must respond with a tool call, not text. "
    "Try a different approach — for example, navigate to a different search engine or a direct URL."
)


def _append_history_openai(messages: list[dict[str, Any]], history: list[StepRecord]) -> None:
    for step in history:
        if step.action == "__text_response__":
            messages.append({"role": "assistant", "content": step.observation})
            messages.append({"role": "user", "content": _TEXT_ONLY_NUDGE})
        elif step.tool_call_id:
            messages.append(
                {
                    "role": "assistant",
                    "tool_calls": [
                        {
                            "id": step.tool_call_id,
                            "type": "function",
                            "function": {
                                "name": step.action,
                                "arguments": json.dumps(step.params),
                            },
                        }
                    ],
                }
            )
            messages.append(
                {
                    "role": "tool",
                    "tool_call_id": step.tool_call_id,
                    "content": step.observation,
                }
            )
        else:
            messages.append(
                {
                    "role": "assistant",
                    "content": f"Action: {step.action}({_format_params(step.params)})",
                }
            )
            messages.append(
                {
                    "role": "user",
                    "content": f"Observation: {step.observation}",
                }
            )


def _append_history_claude(messages: list[dict[str, Any]], history: list[StepRecord]) -> None:
    for step in history:
        if step.action == "__text_response__":
            messages.append({"role": "assistant", "content": step.observation})
            _append_user_content_claude(messages, _TEXT_ONLY_NUDGE)
        elif step.tool_call_id:
            messages.append(
                {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "tool_use",
                            "id": step.tool_call_id,
                            "name": step.action,
                            "input": step.params,
                        }
                    ],
                }
            )
            messages.append(
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "tool_result",
                            "tool_use_id": step.tool_call_id,
                            "content": step.observation,
                        }
                    ],
                }
            )
        else:
            messages.append(
                {
                    "role": "assistant",
                    "content": f"Action: {step.action}({_format_params(step.params)})",
                }
            )
            messages.append(
                {
                    "role": "user",
                    "content": f"Observation: {step.observation}",
                }
            )


def _append_user_content_claude(messages: list[dict[str, Any]], text: str) -> None:
    """Merge into preceding user message when needed — Claude requires alternating roles."""
    if messages and messages[-1]["role"] == "user":
        last_content = messages[-1]["content"]
        if isinstance(last_content, list):
            last_content.append({"type": "text", "text": text})
        else:
            messages[-1]["content"] = last_content + "\n\n" + text
    else:
        messages.append({"role": "user", "content": text})


def _format_params(params: dict[str, Any]) -> str:
    """Format parameters compactly for display."""
    parts = []
    for k, v in params.items():
        if isinstance(v, str) and len(v) > 100:
            v = v[:100] + "..."
        parts.append(f"{k}={v!r}")
    return ", ".join(parts)
