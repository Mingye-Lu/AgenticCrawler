from unittest.mock import patch

import pytest

from agentic_crawler.config import Settings
from agentic_crawler.llm.base import LLMResponse, ToolCall
from agentic_crawler.llm.registry import get_provider
from tests.conftest import MockLLMProvider


@pytest.mark.asyncio
async def test_mock_llm_returns_scripted_responses() -> None:
    provider = MockLLMProvider(responses=[
        LLMResponse(text="Hello world"),
        LLMResponse(tool_calls=[ToolCall(id="1", name="navigate", arguments={"url": "https://example.com"})]),
    ])

    r1 = await provider.complete(messages=[{"role": "user", "content": "hi"}])
    assert r1.text == "Hello world"
    assert not r1.has_tool_calls

    r2 = await provider.complete(messages=[{"role": "user", "content": "go"}])
    assert r2.has_tool_calls
    assert r2.tool_calls[0].name == "navigate"

    # After exhausting scripted responses, returns done
    r3 = await provider.complete(messages=[{"role": "user", "content": "more"}])
    assert r3.has_tool_calls
    assert r3.tool_calls[0].name == "done"


@pytest.mark.asyncio
async def test_mock_llm_logs_messages() -> None:
    provider = MockLLMProvider(responses=[LLMResponse(text="ok")])
    msgs = [{"role": "user", "content": "test"}]
    await provider.complete(messages=msgs)
    assert len(provider.messages_log) == 1
    assert provider.messages_log[0] == msgs


def test_registry_returns_openai_with_api_key() -> None:
    settings = Settings(llm_provider="openai", openai_api_key="sk-test", openai_model="gpt-4o", openai_auth_method="api_key")
    provider = get_provider(settings)
    from agentic_crawler.llm.openai import OpenAIProvider

    assert isinstance(provider, OpenAIProvider)
    assert provider.model == "gpt-4o"
    assert not provider._use_oauth


def test_registry_returns_openai_with_oauth() -> None:
    from agentic_crawler.llm.oauth import OAuthTokens

    fake_tokens = OAuthTokens(access_token="fake-at", refresh_token="fake-rt", expires_at=9999999999.0)
    with patch("agentic_crawler.llm.oauth.load_tokens", return_value=fake_tokens):
        settings = Settings(llm_provider="openai", openai_auth_method="oauth", openai_model="gpt-4o")
        provider = get_provider(settings)
        from agentic_crawler.llm.openai import OpenAIProvider

        assert isinstance(provider, OpenAIProvider)
        assert provider._use_oauth


def test_registry_returns_codex_provider() -> None:
    from agentic_crawler.llm.oauth import OAuthTokens

    fake_tokens = OAuthTokens(access_token="fake-at", refresh_token="fake-rt", expires_at=9999999999.0)
    with patch("agentic_crawler.llm.oauth.load_tokens", return_value=fake_tokens):
        settings = Settings(llm_provider="codex", codex_model="codex-mini-latest")
        provider = get_provider(settings)
        from agentic_crawler.llm.openai import OpenAIProvider

        assert isinstance(provider, OpenAIProvider)
        assert provider.model == "codex-mini-latest"
        assert provider._use_oauth


def test_registry_codex_fails_without_tokens() -> None:
    with patch("agentic_crawler.llm.oauth.load_tokens", return_value=None):
        settings = Settings(llm_provider="codex")
        with pytest.raises(RuntimeError, match="No OAuth tokens found"):
            get_provider(settings)


def test_oauth_pkce_generation() -> None:
    from agentic_crawler.llm.oauth import _generate_pkce

    verifier, challenge = _generate_pkce()
    assert len(verifier) == 64  # 32 bytes hex
    assert len(challenge) > 0
    # Verify S256: challenge == base64url(sha256(verifier))
    import base64
    import hashlib

    digest = hashlib.sha256(verifier.encode("ascii")).digest()
    expected = base64.urlsafe_b64encode(digest).rstrip(b"=").decode("ascii")
    assert challenge == expected


def test_oauth_token_serialization() -> None:
    from agentic_crawler.llm.oauth import OAuthTokens

    tokens = OAuthTokens(access_token="at", refresh_token="rt", expires_at=1700000000.0)
    d = tokens.to_dict()
    assert d["type"] == "oauth"
    assert d["expires"] == 1700000000000  # ms

    restored = OAuthTokens.from_dict(d)
    assert restored.access_token == "at"
    assert restored.refresh_token == "rt"
    assert restored.expires_at == 1700000000.0


def test_oauth_build_authorization_url() -> None:
    from agentic_crawler.llm.oauth import build_authorization_url

    url, verifier, state = build_authorization_url()
    assert "auth.openai.com/oauth/authorize" in url
    assert "code_challenge_method=S256" in url
    assert "response_type=code" in url
    assert len(verifier) > 0
    assert len(state) > 0
