from __future__ import annotations

from typing import Any

from agentic_crawler.actions.base import Action
from agentic_crawler.actions.click import ClickAction
from agentic_crawler.actions.execute_js import ExecuteJsAction
from agentic_crawler.actions.extract import ExtractDataAction
from agentic_crawler.actions.fill_form import FillFormAction
from agentic_crawler.actions.go_back import GoBackAction
from agentic_crawler.actions.hover import HoverAction
from agentic_crawler.actions.navigate import NavigateAction
from agentic_crawler.actions.press_key import PressKeyAction
from agentic_crawler.actions.screenshot import ScreenshotAction
from agentic_crawler.actions.scroll import ScrollAction
from agentic_crawler.actions.select_option import SelectOptionAction
from agentic_crawler.actions.switch_tab import SwitchTabAction
from agentic_crawler.actions.wait import WaitAction


def get_action_registry() -> dict[str, Action]:
    actions: list[Action] = [
        NavigateAction(),
        ClickAction(),
        FillFormAction(),
        ScrollAction(),
        ExtractDataAction(),
        ScreenshotAction(),
        WaitAction(),
        SelectOptionAction(),
        GoBackAction(),
        ExecuteJsAction(),
        HoverAction(),
        PressKeyAction(),
        SwitchTabAction(),
    ]
    return {a.name: a for a in actions}


def get_tool_schemas() -> list[dict[str, Any]]:
    return [
        {
            "name": "navigate",
            "description": "Navigate to a URL. Use this to visit a new page.",
            "parameters": {
                "type": "object",
                "properties": {
                    "url": {"type": "string", "description": "The URL to navigate to"},
                },
                "required": ["url"],
            },
        },
        {
            "name": "click",
            "description": "Click an element on the page. Provide either a CSS selector or text content.",
            "parameters": {
                "type": "object",
                "properties": {
                    "selector": {
                        "type": "string",
                        "description": "CSS selector of the element to click",
                    },
                    "text": {
                        "type": "string",
                        "description": "Visible text of the element to click",
                    },
                },
            },
        },
        {
            "name": "fill_form",
            "description": "Fill form fields and optionally submit. Keys in 'fields' are CSS selectors, values are text to type.",
            "parameters": {
                "type": "object",
                "properties": {
                    "fields": {
                        "type": "object",
                        "description": "Mapping of CSS selector -> value to fill",
                        "additionalProperties": {"type": "string"},
                    },
                    "submit": {
                        "type": "boolean",
                        "description": "Whether to submit the form after filling",
                        "default": False,
                    },
                    "form_selector": {
                        "type": "string",
                        "description": "CSS selector of the form",
                        "default": "form",
                    },
                },
                "required": ["fields"],
            },
        },
        {
            "name": "scroll",
            "description": "Scroll the page up or down to reveal more content.",
            "parameters": {
                "type": "object",
                "properties": {
                    "direction": {
                        "type": "string",
                        "enum": ["up", "down"],
                        "default": "down",
                    },
                    "amount": {
                        "type": "integer",
                        "description": "Pixels to scroll",
                        "default": 500,
                    },
                },
            },
        },
        {
            "name": "extract_data",
            "description": "Extract structured data from the current page. Provide the extracted data directly and an instruction describing what was extracted.",
            "parameters": {
                "type": "object",
                "properties": {
                    "instruction": {
                        "type": "string",
                        "description": "Description of what data is being extracted",
                    },
                    "data": {
                        "description": "The extracted data (any JSON-serializable structure)",
                    },
                },
                "required": ["instruction", "data"],
            },
        },
        {
            "name": "screenshot",
            "description": "Take a screenshot of the current page for visual inspection.",
            "parameters": {
                "type": "object",
                "properties": {},
            },
        },
        {
            "name": "wait",
            "description": "Wait for a CSS selector to appear or for a fixed number of seconds.",
            "parameters": {
                "type": "object",
                "properties": {
                    "selector": {
                        "type": "string",
                        "description": "CSS selector to wait for",
                    },
                    "seconds": {
                        "type": "number",
                        "description": "Seconds to wait (if no selector)",
                        "default": 2,
                    },
                },
            },
        },
        {
            "name": "select_option",
            "description": "Select an option from a <select> dropdown. Provide the selector for the <select> and one of: value, label, or index.",
            "parameters": {
                "type": "object",
                "properties": {
                    "selector": {
                        "type": "string",
                        "description": "CSS selector of the <select> element",
                    },
                    "value": {
                        "type": "string",
                        "description": "The value attribute of the option to select",
                    },
                    "label": {
                        "type": "string",
                        "description": "The visible text of the option to select",
                    },
                    "index": {
                        "type": "integer",
                        "description": "Zero-based index of the option to select",
                    },
                },
                "required": ["selector"],
            },
        },
        {
            "name": "go_back",
            "description": "Navigate back to the previous page in browser history (like the browser Back button).",
            "parameters": {
                "type": "object",
                "properties": {},
            },
        },
        {
            "name": "execute_js",
            "description": "Execute JavaScript in the page context and return the result. Use for computed styles, complex DOM queries, or data the page calculates dynamically.",
            "parameters": {
                "type": "object",
                "properties": {
                    "script": {
                        "type": "string",
                        "description": "JavaScript code to execute (use `return` to get a value back)",
                    },
                },
                "required": ["script"],
            },
        },
        {
            "name": "hover",
            "description": "Hover over an element to reveal hidden content like dropdown menus, tooltips, or mega-menus.",
            "parameters": {
                "type": "object",
                "properties": {
                    "selector": {
                        "type": "string",
                        "description": "CSS selector of the element to hover over",
                    },
                },
                "required": ["selector"],
            },
        },
        {
            "name": "press_key",
            "description": "Press a keyboard key. Useful for Enter (submit), Escape (close modal), Tab (next field), ArrowDown, etc.",
            "parameters": {
                "type": "object",
                "properties": {
                    "key": {
                        "type": "string",
                        "description": "Key to press: Enter, Escape, Tab, ArrowDown, ArrowUp, Backspace, etc.",
                    },
                    "selector": {
                        "type": "string",
                        "description": "Optional CSS selector to focus before pressing the key",
                    },
                },
                "required": ["key"],
            },
        },
        {
            "name": "switch_tab",
            "description": "Switch to a different browser tab. Use when a click opens a new tab. Index -1 for newest, 0 for first.",
            "parameters": {
                "type": "object",
                "properties": {
                    "index": {
                        "type": "integer",
                        "description": "Tab index (-1 for newest, 0 for first)",
                        "default": -1,
                    },
                },
            },
        },
        {
            "name": "done",
            "description": "Signal that the task is complete. Call this when you have finished the goal.",
            "parameters": {
                "type": "object",
                "properties": {
                    "summary": {
                        "type": "string",
                        "description": "Summary of what was accomplished",
                    },
                },
                "required": ["summary"],
            },
        },
    ]
