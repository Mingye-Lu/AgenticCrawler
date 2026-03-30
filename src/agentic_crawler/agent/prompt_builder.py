from __future__ import annotations

import json
from typing import Any

from agentic_crawler.agent.state import AgentState, StepRecord

SYSTEM_PROMPT = """\
You are an autonomous web crawling agent. You navigate websites, interact with pages, \
and extract structured data to accomplish a user's goal.

## Rules
- Think step by step about how to achieve the goal.
- Use the tools provided to interact with web pages.
- When navigating, always use full URLs (including https://).
- When extracting data, provide it in structured JSON format via the extract_data tool.
- If a page requires JavaScript, interactive elements will be available through click, fill_form, etc.
- If you encounter errors, try alternative approaches (different selectors, different URLs).
- When you have accomplished the goal, call the 'done' tool with a summary.
- Do NOT loop indefinitely. If you cannot make progress after several attempts, call 'done' and explain what you found.
- Keep extracted data clean and well-structured.

## Search Strategy
- When searching, simplify the user's goal into clean search keywords. \
Remove underscores, special punctuation, and filename-style formatting. \
For example, for a goal mentioning "Worlds_Together,_Worlds_Apart_A_Companion_Reader volume 2", \
search for: Worlds Together Worlds Apart Companion Reader volume 2.
- If a search returns no results or a "no results" page, try progressively simpler queries: \
drop subtitles, use fewer keywords, try alternate phrasings.
- Do NOT use filetype: operators on DuckDuckGo — they are not supported. \
Instead, include the file type as a keyword (e.g., "pdf" or "epub").
- Try multiple search engines if one fails — both Bing and DuckDuckGo are available.

## Navigation Strategy
- Prefer navigating directly to full URLs rather than filling search forms. \
For example, use navigate(url="https://www.bing.com/search?q=my+query") instead of filling a search box.
- NEVER use Google Search — it blocks automated browsers. Use Bing (bing.com/search?q=...) or DuckDuckGo (duckduckgo.com/?q=...) instead.
- After navigating via a search engine, extract the URLs you need from the page content and navigate directly to those URLs.
- Only use click when you need to interact with a specific page element (buttons, pagination, tabs). \
For following links, prefer navigate with the link's href URL.
- The page content shown to you already contains links with their URLs. Use navigate(url=...) to follow them.
- Use go_back to return to the previous page instead of re-navigating when you need to backtrack.

## Interaction Tools
- Use hover to reveal dropdown menus, tooltips, or mega-menus before clicking items inside them.
- Use select_option for <select> dropdowns (do NOT click individual options — use select_option with value, label, or index).
- Use press_key for keyboard actions: Enter to submit, Escape to close modals, Tab to move between fields, ArrowDown to navigate dropdown lists.
- Use execute_js when you need computed values, want to trigger page events, or need to reach data that CSS selectors cannot express.
- Use switch_tab when a click opens content in a new browser tab — switch to it and then continue working.

## Available Information
Each turn, you will see:
- The current page content (title, text, links, forms)
- Your action history
- The original goal

Choose your next action wisely to make progress toward the goal.
"""

HISTORY_WINDOW = 15  # Max recent steps to include


def build_messages(state: AgentState, provider: str = "claude") -> list[dict[str, Any]]:
    """Build the message list for the LLM from current agent state."""
    messages: list[dict[str, Any]] = [
        {"role": "system", "content": SYSTEM_PROMPT},
    ]

    # Initial user message with the goal
    user_content = f"## Goal\n{state.goal}\n"

    if state.plan:
        user_content += "\n## Plan\n" + "\n".join(
            f"{i + 1}. {step}" for i, step in enumerate(state.plan)
        )

    messages.append({"role": "user", "content": user_content})

    recent_history = state.history[-HISTORY_WINDOW:]
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


def build_plan_messages(goal: str) -> list[dict[str, Any]]:
    return [
        {
            "role": "system",
            "content": (
                "You are a planning agent. Given a web crawling goal, produce a concise step-by-step plan. "
                "Each step should be a short action description. Return ONLY the plan as a numbered list, nothing else.\n\n"
                "Important constraints:\n"
                "- Use Bing or DuckDuckGo for web searches, NEVER Google (it blocks automated browsers).\n"
                "- Navigate directly to URLs when possible instead of filling search forms.\n"
                "- Prefer navigate(url=...) over click for following links.\n"
                "- Simplify search queries: remove underscores, punctuation, and filename-style formatting from the goal. "
                "Use clean, natural keywords. If no results, try simpler/shorter queries."
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
