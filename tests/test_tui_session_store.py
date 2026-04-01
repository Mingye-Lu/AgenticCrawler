"""Tests for tui.session_store — session persistence."""

from __future__ import annotations

from datetime import datetime
from pathlib import Path

import pytest

from agentic_crawler.tui.session_store import (
    ConversationMessage,
    CrawlResult,
    Session,
    SessionStore,
    StepRecord,
)


@pytest.fixture()
def tmp_sessions_dir(tmp_path: Path) -> Path:
    return tmp_path / "sessions"


@pytest.fixture()
def store(tmp_sessions_dir: Path) -> SessionStore:
    return SessionStore(sessions_dir=tmp_sessions_dir)


# ---------- 1. Save and load roundtrip ----------


def test_save_and_load_roundtrip(store: SessionStore) -> None:
    session = Session.create(goal="scrape books")
    session.conversation.append(ConversationMessage(role="user", content="hello"))
    session.crawl_results.append(
        CrawlResult(url="https://example.com", extracted_data={"title": "Ex"})
    )
    session.agent_history.append(
        StepRecord(action="navigate", params={"url": "https://example.com"})
    )

    store.save(session)
    loaded = store.load(session.session_id)

    assert loaded.session_id == session.session_id
    assert loaded.goal == "scrape books"
    assert len(loaded.conversation) == 1
    assert loaded.conversation[0].role == "user"
    assert loaded.conversation[0].content == "hello"
    assert len(loaded.crawl_results) == 1
    assert loaded.crawl_results[0].url == "https://example.com"
    assert len(loaded.agent_history) == 1
    assert loaded.agent_history[0].action == "navigate"


# ---------- 2. list_sessions returns correct count ----------


def test_list_sessions_count(store: SessionStore) -> None:
    for i in range(3):
        s = Session.create(goal=f"goal {i}")
        store.save(s)

    summaries = store.list_sessions()
    assert len(summaries) == 3
    # All goals present
    goals = {s.goal for s in summaries}
    assert goals == {"goal 0", "goal 1", "goal 2"}


# ---------- 3. Corrupted JSON skipped gracefully ----------


def test_corrupted_json_skipped(store: SessionStore, tmp_sessions_dir: Path) -> None:
    # Save a valid session first
    valid = Session.create(goal="valid session")
    store.save(valid)

    # Write a corrupted file
    corrupted_path = tmp_sessions_dir / "corrupted.json"
    corrupted_path.write_text("{invalid json content///")

    with pytest.warns(UserWarning, match="Skipping corrupted session file"):
        summaries = store.list_sessions()

    assert len(summaries) == 1
    assert summaries[0].goal == "valid session"


# ---------- 4. sessions_dir created automatically ----------


def test_sessions_dir_auto_created(tmp_path: Path) -> None:
    new_dir = tmp_path / "deeply" / "nested" / "sessions"
    assert not new_dir.exists()

    _store = SessionStore(sessions_dir=new_dir)
    assert new_dir.exists()
    assert new_dir.is_dir()


# ---------- 5. delete removes session file ----------


def test_delete_session(store: SessionStore, tmp_sessions_dir: Path) -> None:
    session = Session.create(goal="to delete")
    store.save(session)

    path = tmp_sessions_dir / f"{session.session_id}.json"
    assert path.exists()

    store.delete(session.session_id)
    assert not path.exists()


def test_delete_nonexistent_no_error(store: SessionStore) -> None:
    # Should not raise
    store.delete("nonexistent-id")


# ---------- 6. Session.create factory ----------


def test_session_create_factory() -> None:
    session = Session.create(goal="test goal", settings_snapshot={"key": "val"})
    assert session.goal == "test goal"
    assert session.settings_snapshot == {"key": "val"}
    assert session.session_id  # UUID assigned
    assert isinstance(session.created_at, datetime)
    assert session.conversation == []
    assert session.crawl_results == []
    assert session.agent_history == []


def test_session_create_default_settings() -> None:
    session = Session.create(goal="minimal")
    assert session.settings_snapshot == {}


# ---------- 7. Conversation messages serialized ----------


def test_conversation_messages_serialization(store: SessionStore) -> None:
    session = Session.create(goal="chat test")
    session.conversation.append(ConversationMessage(role="user", content="find books"))
    session.conversation.append(ConversationMessage(role="assistant", content="I found 3 books"))

    store.save(session)
    loaded = store.load(session.session_id)

    assert len(loaded.conversation) == 2
    assert loaded.conversation[0].role == "user"
    assert loaded.conversation[0].content == "find books"
    assert loaded.conversation[1].role == "assistant"
    assert loaded.conversation[1].content == "I found 3 books"
    # Timestamps are preserved
    assert isinstance(loaded.conversation[0].timestamp, datetime)


# ---------- 8. load raises FileNotFoundError ----------


def test_load_missing_session_raises(store: SessionStore) -> None:
    with pytest.raises(FileNotFoundError):
        store.load("does-not-exist")


# ---------- 9. list_sessions sorted by mtime, step_count correct ----------


def test_list_sessions_step_count(store: SessionStore) -> None:
    session = Session.create(goal="with steps")
    session.agent_history.append(StepRecord(action="navigate", params={}, observation="ok"))
    session.agent_history.append(
        StepRecord(action="click", params={"selector": "#btn"}, observation="clicked")
    )
    store.save(session)

    summaries = store.list_sessions()
    assert len(summaries) == 1
    assert summaries[0].step_count == 2
    assert summaries[0].session_id == session.session_id
