"""
Production auth module for command-code-proxy.

Handles:
- Token validation on startup and per-request
- Auth state caching with TTL to avoid per-request file reads
- Structured error responses for auth failures
- Health check auth verification
- Support for API key and OAuth authentication
- Graceful degradation when auth files are missing/corrupt
"""

import json
import logging
import os
import threading
import time
from pathlib import Path
from typing import Any, Optional

log = logging.getLogger("command-code-proxy.auth")

AUTH_DIR = Path(os.environ.get("COMMAND_CODE_AUTH_DIR",
                               os.path.expanduser("~/.commandcode")))

# Cache TTL: re-read auth files at most every N seconds
_AUTH_CACHE_TTL = int(os.environ.get("COMMAND_CODE_AUTH_CACHE_TTL", "30"))

# --- Auth state ------------------------------------------------------------


class AuthState:
    """Thread-safe auth state with cached token reads."""

    def __init__(self):
        self._lock = threading.Lock()
        self._auth: Optional[dict[str, Any]] = None
        self._config: Optional[dict[str, Any]] = None
        self._auth_last_read: float = 0.0
        self._config_last_read: float = 0.0
        self._auth_error: Optional[str] = None
        self._config_error: Optional[str] = None

    def get_auth(self, force: bool = False) -> dict[str, Any]:
        """Get auth data, re-reading from disk if cache expired."""
        now = time.time()
        with self._lock:
            if (not force and self._auth is not None
                    and now - self._auth_last_read < _AUTH_CACHE_TTL):
                return self._auth

        # Read outside lock (file I/O)
        auth_file = AUTH_DIR / "auth.json"
        try:
            if not auth_file.exists():
                raise FileNotFoundError(f"Auth file not found: {auth_file}")
            with open(auth_file) as f:
                data = json.load(f)
            # Validate required fields
            if not isinstance(data, dict):
                raise ValueError("Auth file must be a JSON object")
            if "apiKey" not in data and "oauthToken" not in data:
                raise ValueError("Auth file must contain 'apiKey' or 'oauthToken'")
            with self._lock:
                self._auth = data
                self._auth_last_read = now
                self._auth_error = None
            return data
        except Exception as e:
            with self._lock:
                self._auth_error = str(e)
            raise

    def get_config(self, force: bool = False) -> dict[str, Any]:
        """Get config data, re-reading from disk if cache expired."""
        now = time.time()
        with self._lock:
            if (not force and self._config is not None
                    and now - self._config_last_read < _AUTH_CACHE_TTL):
                return self._config

        config_file = AUTH_DIR / "config.json"
        try:
            if not config_file.exists():
                with self._lock:
                    self._config = {}
                    self._config_last_read = now
                    self._config_error = None
                return {}
            with open(config_file) as f:
                data = json.load(f)
            if not isinstance(data, dict):
                data = {}
            with self._lock:
                self._config = data
                self._config_last_read = now
                self._config_error = None
            return data
        except Exception as e:
            with self._lock:
                self._config_error = str(e)
            return {}

    def get_api_key(self) -> Optional[str]:
        """Get API key from auth, or None if using OAuth."""
        auth = self.get_auth()
        return auth.get("apiKey")

    def get_oauth_token(self) -> Optional[str]:
        """Get OAuth token from auth, or None if using API key."""
        auth = self.get_auth()
        return auth.get("oauthToken")

    def get_oauth_provider(self) -> Optional[str]:
        """Get OAuth provider from auth."""
        auth = self.get_auth()
        return auth.get("oauthProvider")

    def has_api_key_auth(self) -> bool:
        """Check if using API key authentication."""
        return bool(self.get_api_key())

    def has_oauth_auth(self) -> bool:
        """Check if using OAuth authentication."""
        return bool(self.get_oauth_token())

    def health_check(self) -> dict[str, Any]:
        """Comprehensive auth health check."""
        result = {
            "auth_dir": str(AUTH_DIR),
            "auth_file_exists": (AUTH_DIR / "auth.json").exists(),
            "config_file_exists": (AUTH_DIR / "config.json").exists(),
            "auth_method": None,
            "auth_valid": False,
            "config_valid": True,
            "error": None,
        }
        try:
            auth = self.get_auth(force=True)
            if auth.get("apiKey"):
                result["auth_method"] = "api_key"
                result["auth_valid"] = len(auth["apiKey"]) > 10
            elif auth.get("oauthToken"):
                result["auth_method"] = "oauth"
                result["auth_valid"] = len(auth["oauthToken"]) > 10
        except Exception as e:
            result["error"] = str(e)

        try:
            cfg = self.get_config(force=True)
            result["config_valid"] = isinstance(cfg, dict)
        except Exception as e:
            result["config_valid"] = False
            result["error"] = str(e)

        return result

    def invalidate(self):
        """Force re-read on next access."""
        with self._lock:
            self._auth = None
            self._config = None
            self._auth_last_read = 0.0
            self._config_last_read = 0.0

    @property
    def last_error(self) -> Optional[str]:
        with self._lock:
            return self._auth_error or self._config_error


# Module-level singleton
_auth_state = AuthState()


def get_auth_state() -> AuthState:
    return _auth_state


# --- Auth header construction ---------------------------------------------


def build_auth_headers(cwd: Optional[str] = None) -> dict[str, str]:
    """Build request headers that exactly match what the `cmd` CLI sends.

    Uses cached auth state (re-reads from disk at most every _AUTH_CACHE_TTL
    seconds) to avoid per-request file I/O.
    """
    from .wire_format import get_cli_version

    auth = _auth_state.get_auth()
    cfg = _auth_state.get_config()

    cli_version = get_cli_version()
    cli_env = os.environ.get("COMMAND_CODE_ENV", "production")
    import os as _os
    project_slug = _os.path.basename(_os.path.normpath(cwd or _os.getcwd()))
    taste_learning = str(cfg.get("tasteLearning", True)).lower()
    oauth_enforced = str(cfg.get("oauthEnforced", False)).lower()

    import uuid
    session_id = str(uuid.uuid4())

    headers = {
        "Content-Type": "application/json",
        "User-Agent": "cli",
        "x-command-code-version": cli_version,
        "x-cli-environment": cli_env,
        "x-project-slug": project_slug,
        "x-taste-learning": taste_learning,
        "x-co-flag": oauth_enforced,
        "x-session-id": session_id,
    }

    api_key = auth.get("apiKey")
    if api_key:
        headers["Authorization"] = f"Bearer {api_key}"

    oauth_token = auth.get("oauthToken")
    if oauth_token:
        headers["x-oauth-token"] = f"Bearer {oauth_token}"
        provider = auth.get("oauthProvider", "")
        if provider:
            headers["x-oauth-provider"] = provider

    return headers
