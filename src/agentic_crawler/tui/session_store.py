"""Session persistence — save/load conversation history and crawl results as JSON."""

from __future__ import annotations

import uuid
import warnings
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

from pydantic import BaseModel, Field


# ---------------------------------------------------------------------------
# Data models
# ---------------------------------------------------------------------------


class ConversationMessage(BaseModel):
    """A single user ↔ assistant message."""

    role: str  # "user" or "assistant"
    content: str
    timestamp: datetime = Field(default_factory=lambda: datetime.now(UTC))


class CrawlResult(BaseModel):
    """Result of visiting / extracting from a single URL."""

    url: str
    extracted_data: dict[str, Any] = Field(default_factory=dict)
    screenshot_path: str | None = None  # file path only — never base64
    success: bool = True


class StepRecord(BaseModel):
    """One agent action with its observation."""

    action: str
    params: dict[str, Any] = Field(default_factory=dict)
    observation: str | None = None
    success: bool = True
    timestamp: datetime = Field(default_factory=lambda: datetime.now(UTC))


class Session(BaseModel):
    """Full session state that gets serialized to disk."""

    session_id: str = Field(default_factory=lambda: str(uuid.uuid4()))
    created_at: datetime = Field(default_factory=lambda: datetime.now(UTC))
    updated_at: datetime = Field(default_factory=lambda: datetime.now(UTC))
    goal: str
    settings_snapshot: dict[str, Any] = Field(default_factory=dict)
    conversation: list[ConversationMessage] = Field(default_factory=list)
    crawl_results: list[CrawlResult] = Field(default_factory=list)
    agent_history: list[StepRecord] = Field(default_factory=list)

    @classmethod
    def create(
        cls,
        goal: str,
        settings_snapshot: dict[str, Any] | None = None,
    ) -> Session:
        """Factory: build a new session with sensible defaults."""
        return cls(goal=goal, settings_snapshot=settings_snapshot or {})


class SessionSummary(BaseModel):
    """Lightweight summary returned by :py:meth:`SessionStore.list_sessions`."""

    session_id: str
    goal: str
    created_at: datetime
    updated_at: datetime
    step_count: int


# ---------------------------------------------------------------------------
# Store
# ---------------------------------------------------------------------------


class SessionStore:
    """Read / write :class:`Session` objects as JSON files on disk."""

    def __init__(self, sessions_dir: Path = Path(".sessions")) -> None:
        self.sessions_dir = sessions_dir
        self.sessions_dir.mkdir(parents=True, exist_ok=True)

    # -- persistence --------------------------------------------------------

    def save(self, session: Session) -> None:
        """Write *session* to ``{sessions_dir}/{session_id}.json``."""
        session.updated_at = datetime.now(UTC)
        path = self.sessions_dir / f"{session.session_id}.json"
        path.write_text(session.model_dump_json(indent=2), encoding="utf-8")

    def load(self, session_id: str) -> Session:
        """Load a session.  Raises :class:`FileNotFoundError` when absent."""
        path = self.sessions_dir / f"{session_id}.json"
        if not path.exists():
            raise FileNotFoundError(f"No session file: {path}")
        return Session.model_validate_json(path.read_text(encoding="utf-8"))

    # -- listing / housekeeping ---------------------------------------------

    def list_sessions(self) -> list[SessionSummary]:
        """Return summaries of all saved sessions (newest first).

        Corrupted files are skipped with a :py:func:`warnings.warn` call.
        """
        summaries: list[SessionSummary] = []
        paths = sorted(
            self.sessions_dir.glob("*.json"),
            key=lambda p: p.stat().st_mtime,
            reverse=True,
        )
        for path in paths:
            try:
                session = Session.model_validate_json(
                    path.read_text(encoding="utf-8"),
                )
                summaries.append(
                    SessionSummary(
                        session_id=session.session_id,
                        goal=session.goal,
                        created_at=session.created_at,
                        updated_at=session.updated_at,
                        step_count=len(session.agent_history),
                    )
                )
            except Exception:
                warnings.warn(
                    f"Skipping corrupted session file: {path}",
                    UserWarning,
                    stacklevel=2,
                )
        return summaries

    def delete(self, session_id: str) -> None:
        """Delete a session file (no-op if already gone)."""
        path = self.sessions_dir / f"{session_id}.json"
        path.unlink(missing_ok=True)
