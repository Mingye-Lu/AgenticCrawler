from __future__ import annotations

from typing import Any

from agentic_crawler.agent.state import AgentState

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

## Available Information
Each turn, you will see:
- The current page content (title, text, links, forms)
- Your action history
- The original goal

Choose your next action wisely to make progress toward the goal.
"""

HISTORY_WINDOW = 15  # Max recent steps to include


def build_messages(state: AgentState) -> list[dict[str, Any]]:
    """Build the message list for the LLM from current agent state."""
    messages: list[dict[str, Any]] = [
        {"role": "system", "content": SYSTEM_PROMPT},
    ]

    # Initial user message with the goal
    user_content = f"## Goal\n{state.goal}\n"

    if state.plan:
        user_content += "\n## Plan\n" + "\n".join(f"{i+1}. {step}" for i, step in enumerate(state.plan))

    messages.append({"role": "user", "content": user_content})

    # Add history as assistant/user turn pairs (action -> observation)
    recent_history = state.history[-HISTORY_WINDOW:]
    for step in recent_history:
        # Assistant turn: the action taken
        messages.append({
            "role": "assistant",
            "content": f"Action: {step.action}({_format_params(step.params)})",
        })
        # User turn: the observation
        messages.append({
            "role": "user",
            "content": f"Observation: {step.observation}",
        })

    # Current page context
    if state.page_summary:
        page_msg = f"## Current Page\n{state.page_summary}"
        if state.extracted_data:
            page_msg += f"\n\n## Data Extracted So Far\n{len(state.extracted_data)} item(s) collected."
        page_msg += "\n\nWhat is your next action?"
        messages.append({"role": "user", "content": page_msg})
    elif not state.history:
        messages.append({"role": "user", "content": "You have not visited any page yet. Start by navigating to a relevant URL."})

    return messages


def build_plan_messages(goal: str) -> list[dict[str, Any]]:
    """Build messages for the initial planning step."""
    return [
        {
            "role": "system",
            "content": (
                "You are a planning agent. Given a web crawling goal, produce a concise step-by-step plan. "
                "Each step should be a short action description. Return ONLY the plan as a numbered list, nothing else."
            ),
        },
        {"role": "user", "content": f"Goal: {goal}\n\nProduce a step-by-step plan:"},
    ]


def _format_params(params: dict[str, Any]) -> str:
    """Format parameters compactly for display."""
    parts = []
    for k, v in params.items():
        if isinstance(v, str) and len(v) > 100:
            v = v[:100] + "..."
        parts.append(f"{k}={v!r}")
    return ", ".join(parts)
