from agentic_crawler.config import Settings


def test_default_fork_settings():
    """Verify all 5 fork limit defaults are correct."""
    settings = Settings(_env_file=None)

    assert settings.max_concurrent_per_parent == 5
    assert settings.max_fork_depth == 3
    assert settings.max_total_agents == 10
    assert settings.fork_child_max_steps == 15
    assert settings.fork_wait_timeout == 60


def test_fork_settings_from_env(monkeypatch):
    """Set env vars and verify they're picked up by Settings."""
    monkeypatch.setenv("MAX_CONCURRENT_PER_PARENT", "3")
    monkeypatch.setenv("MAX_FORK_DEPTH", "2")
    monkeypatch.setenv("MAX_TOTAL_AGENTS", "8")
    monkeypatch.setenv("FORK_CHILD_MAX_STEPS", "20")
    monkeypatch.setenv("FORK_WAIT_TIMEOUT", "90")

    settings = Settings(_env_file=None)

    assert settings.max_concurrent_per_parent == 3
    assert settings.max_fork_depth == 2
    assert settings.max_total_agents == 8
    assert settings.fork_child_max_steps == 20
    assert settings.fork_wait_timeout == 90
