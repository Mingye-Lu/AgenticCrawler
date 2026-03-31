from __future__ import annotations

import json
from typing import Any

from pydantic import ValidationError


def validate_extracted_data(data: Any, schema: dict[str, Any] | None = None) -> Any:
    """Validate and clean LLM-extracted data.

    If a schema dict is provided, attempts to validate against it.
    Otherwise returns the data as-is after basic cleaning.
    """
    if isinstance(data, str):
        try:
            data = json.loads(data)
        except json.JSONDecodeError:
            return data

    if schema is None:
        return data

    # Build a dynamic Pydantic model from the schema if possible
    try:
        if isinstance(data, list):
            return [_validate_item(item, schema) for item in data]
        return _validate_item(data, schema)
    except (ValidationError, Exception):
        return data


def _validate_item(item: Any, schema: dict[str, Any]) -> Any:
    """Validate a single item against a schema dict."""
    if not isinstance(item, dict):
        return item

    # Simple field-level validation: ensure required keys exist
    properties = schema.get("properties", {})
    required = set(schema.get("required", []))

    cleaned: dict[str, Any] = {}
    for key, prop in properties.items():
        if key in item:
            cleaned[key] = item[key]
        elif key in required:
            cleaned[key] = None  # Mark missing required fields

    # Include any extra fields not in schema
    for key, value in item.items():
        if key not in cleaned:
            cleaned[key] = value

    return cleaned
