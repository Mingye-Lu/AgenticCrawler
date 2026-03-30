"""OAuth 2.0 PKCE flow for OpenAI Codex authentication."""

from __future__ import annotations

import asyncio
import hashlib
import json
import secrets
import time
import webbrowser
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from threading import Thread
from typing import Any
from urllib.parse import parse_qs, urlencode, urlparse

import httpx

# OpenAI Codex OAuth constants
OPENAI_CLIENT_ID = "app_EMoamEEZ73f0CkXaXp7hrann"
OPENAI_AUTH_URL = "https://auth.openai.com/oauth/authorize"
OPENAI_TOKEN_URL = "https://auth.openai.com/oauth/token"
CALLBACK_PORT = 1455
REDIRECT_URI = f"http://localhost:{CALLBACK_PORT}/auth/callback"
SCOPES = "openid profile email offline_access"

# Token storage
DEFAULT_TOKEN_DIR = Path.home() / ".codex"
DEFAULT_TOKEN_FILE = DEFAULT_TOKEN_DIR / "auth.json"

# Refresh tokens 5 minutes before expiry
TOKEN_REFRESH_MARGIN_SECONDS = 300


@dataclass
class OAuthTokens:
    access_token: str
    refresh_token: str
    expires_at: float  # Unix timestamp in seconds

    @property
    def is_expired(self) -> bool:
        return time.time() >= (self.expires_at - TOKEN_REFRESH_MARGIN_SECONDS)

    def to_dict(self) -> dict[str, Any]:
        return {
            "type": "oauth",
            "access": self.access_token,
            "refresh": self.refresh_token,
            "expires": int(self.expires_at * 1000),  # Store as ms for Codex CLI compat
        }

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> OAuthTokens:
        return cls(
            access_token=data["access"],
            refresh_token=data["refresh"],
            expires_at=data["expires"] / 1000,  # Convert ms to seconds
        )


def _generate_pkce() -> tuple[str, str]:
    """Generate PKCE code verifier and challenge (S256)."""
    verifier_bytes = secrets.token_bytes(32)
    code_verifier = verifier_bytes.hex()
    digest = hashlib.sha256(code_verifier.encode("ascii")).digest()
    # base64url encode without padding
    code_challenge = __import__("base64").urlsafe_b64encode(digest).rstrip(b"=").decode("ascii")
    return code_verifier, code_challenge


def build_authorization_url() -> tuple[str, str, str]:
    """Build the OAuth authorization URL.

    Returns (url, code_verifier, state).
    """
    code_verifier, code_challenge = _generate_pkce()
    state = secrets.token_urlsafe(32)

    params = {
        "response_type": "code",
        "client_id": OPENAI_CLIENT_ID,
        "redirect_uri": REDIRECT_URI,
        "scope": SCOPES,
        "state": state,
        "code_challenge": code_challenge,
        "code_challenge_method": "S256",
        "id_token_add_organizations": "true",
        "codex_cli_simplified_flow": "true",
    }
    url = f"{OPENAI_AUTH_URL}?{urlencode(params)}"
    return url, code_verifier, state


class _CallbackHandler(BaseHTTPRequestHandler):
    """HTTP handler that captures the OAuth callback."""

    authorization_code: str | None = None
    callback_state: str | None = None
    error: str | None = None

    def do_GET(self) -> None:  # noqa: N802
        parsed = urlparse(self.path)
        params = parse_qs(parsed.query)

        if "error" in params:
            _CallbackHandler.error = params["error"][0]
            self._respond("Authentication failed. You can close this tab.")
            return

        _CallbackHandler.authorization_code = params.get("code", [None])[0]
        _CallbackHandler.callback_state = params.get("state", [None])[0]
        self._respond("Authentication successful! You can close this tab.")

    def _respond(self, message: str) -> None:
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.end_headers()
        html = f"<html><body><h2>{message}</h2></body></html>"
        self.wfile.write(html.encode())

    def log_message(self, format: str, *args: Any) -> None:  # noqa: A002
        pass  # Suppress HTTP server logs


def _wait_for_callback(expected_state: str, timeout: float = 120) -> str:
    """Start a local HTTP server and wait for the OAuth callback.

    Returns the authorization code.
    """
    # Reset class state
    _CallbackHandler.authorization_code = None
    _CallbackHandler.callback_state = None
    _CallbackHandler.error = None

    server = HTTPServer(("localhost", CALLBACK_PORT), _CallbackHandler)
    server.timeout = timeout

    thread = Thread(target=server.handle_request, daemon=True)
    thread.start()
    thread.join(timeout=timeout)
    server.server_close()

    if _CallbackHandler.error:
        raise RuntimeError(f"OAuth error: {_CallbackHandler.error}")

    if not _CallbackHandler.authorization_code:
        raise TimeoutError("OAuth callback timed out — no authorization code received.")

    if _CallbackHandler.callback_state != expected_state:
        raise ValueError("OAuth state mismatch — possible CSRF attack.")

    return _CallbackHandler.authorization_code


async def exchange_code_for_tokens(code: str, code_verifier: str) -> OAuthTokens:
    """Exchange authorization code for access and refresh tokens."""
    async with httpx.AsyncClient() as client:
        resp = await client.post(
            OPENAI_TOKEN_URL,
            data={
                "grant_type": "authorization_code",
                "code": code,
                "redirect_uri": REDIRECT_URI,
                "client_id": OPENAI_CLIENT_ID,
                "code_verifier": code_verifier,
            },
            headers={"Content-Type": "application/x-www-form-urlencoded"},
        )
        resp.raise_for_status()
        data = resp.json()

    return OAuthTokens(
        access_token=data["access_token"],
        refresh_token=data["refresh_token"],
        expires_at=time.time() + data.get("expires_in", 3600),
    )


async def refresh_access_token(refresh_token: str) -> OAuthTokens:
    """Use a refresh token to obtain a new access token."""
    async with httpx.AsyncClient() as client:
        resp = await client.post(
            OPENAI_TOKEN_URL,
            data={
                "grant_type": "refresh_token",
                "refresh_token": refresh_token,
                "client_id": OPENAI_CLIENT_ID,
            },
            headers={"Content-Type": "application/x-www-form-urlencoded"},
        )
        resp.raise_for_status()
        data = resp.json()

    return OAuthTokens(
        access_token=data["access_token"],
        refresh_token=data.get("refresh_token", refresh_token),
        expires_at=time.time() + data.get("expires_in", 3600),
    )


def save_tokens(tokens: OAuthTokens, path: Path = DEFAULT_TOKEN_FILE) -> None:
    """Persist tokens to disk."""
    path.parent.mkdir(parents=True, exist_ok=True)
    store: dict[str, Any] = {}
    if path.exists():
        try:
            store = json.loads(path.read_text())
        except (json.JSONDecodeError, OSError):
            store = {}
    store["openai-codex"] = tokens.to_dict()
    path.write_text(json.dumps(store, indent=2))


def load_tokens(path: Path = DEFAULT_TOKEN_FILE) -> OAuthTokens | None:
    """Load tokens from disk. Returns None if not found or invalid."""
    if not path.exists():
        return None
    try:
        store = json.loads(path.read_text())
        entry = store.get("openai-codex")
        if not entry:
            return None
        return OAuthTokens.from_dict(entry)
    except (json.JSONDecodeError, KeyError, OSError):
        return None


async def ensure_valid_tokens(path: Path = DEFAULT_TOKEN_FILE) -> OAuthTokens:
    """Load tokens and refresh if expired. Raises if no tokens stored."""
    tokens = load_tokens(path)
    if tokens is None:
        raise RuntimeError("No OAuth tokens found. Run `agentic-crawler login` to authenticate.")
    if tokens.is_expired:
        tokens = await refresh_access_token(tokens.refresh_token)
        save_tokens(tokens, path)
    return tokens


def run_login_flow() -> OAuthTokens:
    """Run the full interactive OAuth login flow (blocking).

    Opens a browser for the user to authorize, waits for the callback,
    exchanges the code, and stores the tokens.
    """
    url, code_verifier, state = build_authorization_url()
    print(f"Opening browser for OpenAI authentication...\n{url}\n")
    webbrowser.open(url)

    code = _wait_for_callback(expected_state=state)
    tokens = asyncio.run(exchange_code_for_tokens(code, code_verifier))
    save_tokens(tokens)
    return tokens
