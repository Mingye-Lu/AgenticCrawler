from __future__ import annotations

import re
from pathlib import Path, PurePosixPath
from typing import Any
from urllib.parse import urlparse

from agentic_crawler.actions.base import ActionResult
from agentic_crawler.fetcher.router import FetcherRouter

_UNSAFE_RE = re.compile(r'[<>:"|?*\x00]')


def _sanitize_filename(name: str) -> str:
    name = name.replace("\\", "/")
    name = PurePosixPath(name).name  # strip directory components
    name = _UNSAFE_RE.sub("_", name)
    name = name.lstrip(".")  # no hidden files
    return name or "download"


def _deduplicate(path: Path) -> Path:
    if not path.exists():
        return path
    stem = path.stem
    suffix = path.suffix
    parent = path.parent
    counter = 1
    while True:
        candidate = parent / f"{stem}_{counter}{suffix}"
        if not candidate.exists():
            return candidate
        counter += 1


class SaveFileAction:
    name = "save_file"
    description = "Download a URL and save it to the workspace directory"

    async def execute(self, router: FetcherRouter, params: dict[str, Any]) -> ActionResult:
        url = params.get("url", "")
        if not url:
            return ActionResult(success=False, observation="url is required")

        filename = params.get("filename", "")
        subdir = params.get("subdir", "")

        # Derive filename from URL if not provided
        if not filename:
            parsed = urlparse(url)
            filename = PurePosixPath(parsed.path).name or "index.html"
        filename = _sanitize_filename(filename)

        # Sanitize subdir
        if subdir:
            subdir = subdir.replace("\\", "/").strip("/")

        # Build and validate output path
        workspace = router.workspace_dir
        target = (workspace / subdir / filename).resolve() if subdir else (workspace / filename).resolve()
        if not target.is_relative_to(workspace):
            return ActionResult(success=False, observation="Path traversal detected: path escapes workspace")

        target = _deduplicate(target)
        target.parent.mkdir(parents=True, exist_ok=True)

        # Download
        try:
            response = await router.http.client.get(url)
            response.raise_for_status()
        except Exception as e:
            return ActionResult(success=False, observation=f"Download failed: {e}")

        target.write_bytes(response.content)
        size = len(response.content)
        rel = target.relative_to(workspace)

        return ActionResult(
            success=True,
            observation=f"Saved {url} -> {rel} ({size} bytes)",
        )
