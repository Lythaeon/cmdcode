"""
Wire-format helpers for command-code-proxy.

Single source of truth for:
  - Auth header construction (mirrors the official `cmd` CLI fingerprint)
  - Git/project context building (cached with short TTL)
  - OpenAI <-> Command Code message/tool translation
  - CLI version detection and self-update (background, non-blocking)
  - Model catalog (auto-parsed from CLI's bundled models.md)
  - Reasoning effort support (low/medium/high/xhigh/max)
"""

import json
import os
import re
import subprocess
import sys
import threading
import time
import uuid
from pathlib import Path
from typing import Any, Optional

# --- Configuration --------------------------------------------------------

COMMAND_CODE_BIN = os.environ.get("COMMAND_CODE_BIN", "command-code")
AUTH_DIR = Path.home() / ".commandcode"

_COMMAND_CODE_API_BASE = os.environ.get("COMMAND_CODE_API_BASE")
if _COMMAND_CODE_API_BASE:
    API_BASE = _COMMAND_CODE_API_BASE
elif os.environ.get("COMMAND_CODE_LOCAL") == "1":
    API_BASE = "http://localhost:9090"
else:
    API_BASE = "https://api.commandcode.ai"

# Reasoning effort levels supported by the API
REASONING_EFFORTS = {"low", "medium", "high", "xhigh", "max"}

DEFAULT_MODEL = "gpt-5.6-luna"

# --- Model catalog (auto-parsed from CLI bundled models.md) ----------------

_MODEL_CATALOG_CACHE: Optional[dict[str, dict[str, Any]]] = None
_MODEL_CATALOG_LOCK = threading.Lock()


def _find_models_md() -> Optional[Path]:
    """Find the CLI's bundled models.md file."""
    # Try the global npm install location
    try:
        result = subprocess.run(
            ["node", "-e",
             "console.log(require.resolve('command-code/package.json'))"],
            capture_output=True, text=True, timeout=5,
        )
        if result.returncode == 0 and result.stdout.strip():
            pkg_json = Path(result.stdout.strip())
            # Walk up to find dist/bundled
            candidate = (pkg_json.parent / "dist" / "bundled" /
                         "command-code-knowledge" / "reference" / "models.md")
            if candidate.exists():
                return candidate
    except Exception:
        pass
    # Try common locations
    for base in [
        Path.home() / ".linuxbrew" / "lib" / "node_modules",
        Path("/home/linuxbrew/.linuxbrew/lib/node_modules"),
        Path("/usr/local/lib/node_modules"),
        Path("/usr/lib/node_modules"),
    ]:
        candidate = (base / "command-code" / "dist" / "bundled" /
                     "command-code-knowledge" / "reference" / "models.md")
        if candidate.exists():
            return candidate
    return None


def _parse_models_md(content: str) -> dict[str, dict[str, Any]]:
    """Parse the CLI's models.md table into a model catalog dict."""
    catalog: dict[str, dict[str, Any]] = {}
    current_provider = "unknown"

    for line in content.split("\n"):
        line = line.strip()

        # Detect provider section headers (## Open Source, ## Anthropic, etc.)
        if line.startswith("## "):
            current_provider = line[3:].strip().lower()
            # Normalize provider names
            provider_map = {
                "open source": "open-source",
                "openai": "openai",
                "anthropic": "anthropic",
                "google": "google",
                "sakana": "sakana",
                "meta": "meta",
                "xiai": "xai",
            }
            current_provider = provider_map.get(current_provider, current_provider)
            continue

        # Parse table rows: | `id` | Name | Context | Efforts | Pricing | Min plan | Best for |
        if not line.startswith("|") or line.startswith("|---") or line.startswith("| Id"):
            continue

        cols = [c.strip() for c in line.split("|")]
        cols = [c for c in cols if c]  # remove empty from leading/trailing |

        if len(cols) < 5:
            continue

        # Extract model ID (backtick-wrapped)
        id_match = re.search(r"`([^`]+)`", cols[0])
        if not id_match:
            continue
        model_id = id_match.group(1)

        name = cols[1] if len(cols) > 1 else model_id
        context = cols[2] if len(cols) > 2 else ""
        efforts_str = cols[3] if len(cols) > 3 else ""

        # Parse efforts
        efforts = []
        if efforts_str and efforts_str != "—":
            efforts = [e.strip() for e in efforts_str.split(",") if e.strip()]

        # Parse context window
        context_window = 0
        if context and context != "—":
            ctx_match = re.search(r"([\d.]+)\s*([KkMm])?", context)
            if ctx_match:
                val = float(ctx_match.group(1))
                unit = (ctx_match.group(2) or "").upper()
                if unit == "M":
                    context_window = int(val * 1_000_000)
                elif unit == "K":
                    context_window = int(val * 1_000)
                else:
                    context_window = int(val)

        catalog[model_id] = {
            "name": name,
            "reasoning": len(efforts) > 0,
            "efforts": efforts,
            "context_window": context_window,
            "provider": current_provider,
        }

    return catalog


def get_model_catalog() -> dict[str, dict[str, Any]]:
    """Get the model catalog. Parsed from CLI's models.md, cached after first call."""
    global _MODEL_CATALOG_CACHE
    if _MODEL_CATALOG_CACHE is not None:
        return _MODEL_CATALOG_CACHE
    with _MODEL_CATALOG_LOCK:
        if _MODEL_CATALOG_CACHE is not None:
            return _MODEL_CATALOG_CACHE
        models_md = _find_models_md()
        if models_md:
            try:
                content = models_md.read_text()
                _MODEL_CATALOG_CACHE = _parse_models_md(content)
                print(f"[command-code-proxy] loaded {len(_MODEL_CATALOG_CACHE)} models "
                      f"from {models_md}", file=sys.stderr, flush=True)
                return _MODEL_CATALOG_CACHE
            except Exception as e:
                print(f"[command-code-proxy] failed to parse models.md: {e}",
                      file=sys.stderr, flush=True)
        # Fallback: empty catalog (proxy still works, just /v1/models is empty)
        _MODEL_CATALOG_CACHE = {}
        return _MODEL_CATALOG_CACHE


def parse_model_and_effort(model_str: str) -> tuple[str, Optional[str]]:
    """Parse 'model_id:effort' into (model_id, effort).

    Examples:
        "gpt-5.6-luna" -> ("gpt-5.6-luna", None)
        "claude-sonnet-5:high" -> ("claude-sonnet-5", "high")
        "command-code/gpt-5.6-luna:max" -> ("gpt-5.6-luna", "max")
    """
    # Strip command-code/ prefix
    if model_str.startswith("command-code/"):
        model_str = model_str[len("command-code/"):]

    # Split on : for effort
    if ":" in model_str:
        parts = model_str.rsplit(":", 1)
        model_id = parts[0]
        effort = parts[1].lower()
        if effort not in REASONING_EFFORTS:
            # Not a valid effort — treat the whole thing as model ID
            return model_str, None
        return model_id, effort

    return model_str, None


# --- Auth / config helpers ------------------------------------------------


def load_auth() -> dict[str, Any]:
    """Read ~/.commandcode/auth.json (re-read on every call so keys stay fresh)."""
    auth_file = AUTH_DIR / "auth.json"
    if not auth_file.exists():
        raise FileNotFoundError(
            f"Command Code auth file not found at {auth_file}. "
            "Run `command-code login` first."
        )
    with open(auth_file) as f:
        return json.load(f)


def load_config() -> dict[str, Any]:
    """Read ~/.commandcode/config.json (re-read on every call)."""
    config_file = AUTH_DIR / "config.json"
    if config_file.exists():
        with open(config_file) as f:
            return json.load(f)
    return {}


# --- CLI version detection (cached) ---------------------------------------

_cli_version_cache: Optional[str] = None
_cli_version_lock = threading.Lock()


def get_cli_version() -> str:
    """Read the installed CLI version. Cached after first call."""
    global _cli_version_cache
    if _cli_version_cache is not None:
        return _cli_version_cache
    with _cli_version_lock:
        if _cli_version_cache is not None:
            return _cli_version_cache
        _cli_version_cache = _detect_cli_version()
        return _cli_version_cache


def _detect_cli_version() -> str:
    """Detect CLI version from npm or CLI binary."""
    try:
        result = subprocess.run(
            ["node", "-e",
             "const p = require('command-code/package.json'); console.log(p.version)"],
            capture_output=True, text=True, timeout=5,
        )
        if result.returncode == 0 and result.stdout.strip():
            return result.stdout.strip()
    except Exception:
        pass
    try:
        result = subprocess.run(
            [COMMAND_CODE_BIN, "--version", "-v"],
            capture_output=True, text=True, timeout=5,
        )
        for line in result.stdout.split("\n"):
            if line.strip().startswith("v") or line.strip().replace(".", "").isdigit():
                return line.strip().lstrip("v")
    except Exception:
        pass
    return "1.0.0"


# --- Self-update (background, non-blocking) --------------------------------

_UPDATE_INTERVAL = 6 * 3600
_update_state = {"checked_at": 0.0, "updating": False}
_update_lock = threading.Lock()

_UPDATE_SCRIPT_TEMPLATE = r"""
PREFIX=$(npm prefix -g) || exit 11
test -n "$PREFIX" || exit 12
NM="$PREFIX/lib/node_modules"
PKG="$NM/{r}"
STAGE="$NM/.{r}-stage-$$"
OLD="$NM/.{r}-old-$$"
if [ ! -d "$PKG" ]; then
  for orphan in "$NM"/.{r}-old-*; do
    [ -d "$orphan" ] && mv "$orphan" "$PKG" 2>/dev/null && break
  done
fi
for d in "$NM"/.{r}-stage-* "$NM"/.{r}-old-*; do
  [ -d "$d" ] && rm -rf "$d" 2>/dev/null
done
trap 'rm -rf "$STAGE" 2>/dev/null || true' EXIT
mkdir -p "$STAGE" || exit 13
npm install -g --prefix "$STAGE" --no-audit --no-fund --no-progress {r}@{e} || exit 14
test -f "$STAGE/lib/node_modules/{r}/package.json" || exit 15
node -e "process.exit(require(process.argv[1]).version==='{e}'?0:1)" "$STAGE/lib/node_modules/{r}/package.json" 2>/dev/null || exit 15
HAD_OLD=0
if [ -d "$PKG" ]; then
  mv "$PKG" "$OLD" || exit 16
  HAD_OLD=1
fi
if ! mv "$STAGE/lib/node_modules/{r}" "$PKG"; then
  if [ "$HAD_OLD" = "1" ]; then mv "$OLD" "$PKG" 2>/dev/null || cp -R "$OLD" "$PKG" 2>/dev/null; fi
  exit 17
fi
rm -rf "$OLD" 2>/dev/null || true
"""


def _semver_tuple(v: str) -> Optional[tuple]:
    m = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)", v.strip())
    if not m:
        return None
    return tuple(int(g) for g in m.groups())


def _npm_versions(pkg: str) -> list:
    try:
        result = subprocess.run(
            ["npm", "view", pkg, "versions", "--json"],
            capture_output=True, text=True, timeout=5,
            env={**os.environ, "NODE_NO_WARNINGS": "1"},
        )
        if result.returncode != 0:
            return []
        data = json.loads(result.stdout)
        if not isinstance(data, list):
            return []
        releases = []
        for v in data:
            t = _semver_tuple(str(v))
            if t is not None:
                releases.append((t, str(v)))
        releases.sort()
        return [v for _, v in releases]
    except Exception:
        return []


def _do_cli_update() -> None:
    """Check npm registry for newer CLI; install via staged swap. Never raises."""
    if os.environ.get("CI") or os.environ.get("COMMANDCODE_SKIP_UPDATES"):
        return
    now = time.time()
    with _update_lock:
        if now - _update_state["checked_at"] < _UPDATE_INTERVAL:
            return
        if _update_state["updating"]:
            return
        _update_state["checked_at"] = now
        _update_state["updating"] = True
    try:
        current = get_cli_version()
        releases = _npm_versions("command-code")
        if not releases:
            return
        latest = releases[-1]
        latest_t, current_t = _semver_tuple(latest), _semver_tuple(current)
        if latest_t is None or current_t is None or latest_t <= current_t:
            return
        print(f"[command-code-proxy] update available: {current} -> {latest}",
              file=sys.stderr, flush=True)
        script = _UPDATE_SCRIPT_TEMPLATE.format(r="command-code", e=latest)
        result = subprocess.run(
            ["bash", "-c", script],
            capture_output=True, text=True, timeout=600,
            env={**os.environ, "NODE_NO_WARNINGS": "1"},
        )
        if result.returncode == 0:
            global _cli_version_cache
            _cli_version_cache = None
            print(f"[command-code-proxy] updated to {get_cli_version()}",
                  file=sys.stderr, flush=True)
        else:
            print(f"[command-code-proxy] update failed (exit {result.returncode}): "
                  f"{result.stderr.strip()[-400:]}", file=sys.stderr, flush=True)
    except Exception as e:
        print(f"[command-code-proxy] update check error: {e}",
              file=sys.stderr, flush=True)
    finally:
        _update_state["updating"] = False


def ensure_cli_updated_background() -> None:
    """Fire-and-forget background CLI update check. Non-blocking."""
    if os.environ.get("CI") or os.environ.get("COMMANDCODE_SKIP_UPDATES"):
        return
    now = time.time()
    with _update_lock:
        if now - _update_state["checked_at"] < _UPDATE_INTERVAL:
            return
        if _update_state["updating"]:
            return
    threading.Thread(target=_do_cli_update, daemon=True).start()


# --- Auth header construction ---------------------------------------------


def build_auth_headers(cwd: Optional[str] = None) -> dict[str, str]:
    """Build request headers that exactly match what the `cmd` CLI sends."""
    auth = load_auth()
    cfg = load_config()
    runtime_cwd = cwd or os.getcwd()

    cli_version = get_cli_version()
    cli_env = os.environ.get("COMMAND_CODE_ENV", "production")
    project_slug = os.path.basename(os.path.normpath(runtime_cwd))
    taste_learning = str(cfg.get("tasteLearning", True)).lower()
    oauth_enforced = str(cfg.get("oauthEnforced", False)).lower()
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


# --- Git / project context (cached with TTL) -------------------------------

_EXCLUDED_DIRS = {
    "node_modules", "dist", "build", ".git", ".svn", ".hg", "coverage",
    ".nyc_output", ".cache", "tmp", "temp", ".next", ".nuxt", "out",
}

_git_cache: dict[str, tuple[float, dict]] = {}
_git_cache_lock = threading.Lock()
_GIT_CACHE_TTL = 30  # seconds


def _shell(cwd: str, *args: str, timeout: int = 5) -> str:
    try:
        result = subprocess.run(
            ["git", *args], capture_output=True, text=True, cwd=cwd, timeout=timeout,
        )
        if result.returncode != 0:
            return ""
        return result.stdout.strip()
    except Exception:
        return ""


def get_git_info(cwd: str) -> dict[str, Any]:
    """Build the config block exactly like the CLI's buildServerConfig. Cached 30s."""
    now = time.time()
    with _git_cache_lock:
        if cwd in _git_cache:
            ts, data = _git_cache[cwd]
            if now - ts < _GIT_CACHE_TTL:
                return data

    info = _build_git_info(cwd)

    with _git_cache_lock:
        _git_cache[cwd] = (now, info)

    return info


def _build_git_info(cwd: str) -> dict[str, Any]:
    structure = [
        e for e in sorted(os.listdir(cwd))
        if not e.startswith(".") and e not in _EXCLUDED_DIRS
    ] if os.path.isdir(cwd) else []

    info: dict[str, Any] = {
        "workingDir": cwd,
        "date": time.strftime("%Y-%m-%d"),
        "environment": "linux",
        "structure": structure,
        "isGitRepo": False,
        "currentBranch": "",
        "mainBranch": "",
        "gitStatus": "",
        "recentCommits": [],
    }

    if not _shell(cwd, "rev-parse", "--git-dir"):
        return info

    main_branch = _shell(cwd, "symbolic-ref", "--short", "refs/remotes/origin/HEAD")
    if main_branch:
        main_branch = main_branch.replace("origin/", "")
    if not main_branch:
        branches = _shell(cwd, "branch", "-r")
        main_branch = "main" if "origin/main" in branches else (
            "master" if "origin/master" in branches else "main"
        )

    info["isGitRepo"] = True
    info["currentBranch"] = _shell(cwd, "branch", "--show-current")
    info["mainBranch"] = main_branch
    status = _shell(cwd, "status", "--porcelain")
    info["gitStatus"] = status or "Working tree clean"
    commits = _shell(cwd, "log", "--oneline", "-3")
    info["recentCommits"] = commits.split("\n") if commits else []

    return info


# --- Message / tool translation -------------------------------------------


def wire_tools(tools: Optional[list[dict]]) -> list[dict]:
    """Convert tool definitions to the CLI's wire format.

    Accepts both CLI wire format [{name, description, input_schema}] and
    OpenAI-style [{type:"function",function:{name,parameters}}].
    """
    if not tools:
        return []
    out = []
    for t in tools:
        if t.get("type") == "function" and isinstance(t.get("function"), dict):
            fn = t["function"]
            out.append({
                "name": fn.get("name", ""),
                "description": fn.get("description", "") or "",
                "input_schema": fn.get("parameters") or {"type": "object", "properties": {}},
            })
        else:
            out.append({
                "name": t.get("name", ""),
                "description": t.get("description", "") or "",
                "input_schema": t.get("input_schema")
                or t.get("parameters")
                or {"type": "object", "properties": {}},
            })
    return out


def wire_messages(messages: list[dict]) -> list[dict]:
    """Convert OpenAI-ish messages to the CLI's toWireMessages format."""
    wire: list[dict] = []
    tool_name_map: dict[str, str] = {}

    for msg in messages:
        role = msg.get("role")
        content = msg.get("content")
        if role == "system":
            wire.append({"role": "system", "content": content})
            continue
        if role == "user":
            if isinstance(content, str):
                wire.append({"role": "user", "content": [{"type": "text", "text": content}]})
                continue
            items = []
            for part in content or []:
                if isinstance(part, str):
                    items.append({"type": "text", "text": part})
                elif isinstance(part, dict):
                    if part.get("type") == "text":
                        items.append({"type": "text", "text": part.get("text", "")})
                    elif part.get("type") in ("image", "image_url"):
                        url = part.get("image") or part.get("image_url", {}).get("url", "")
                        items.append({"type": "image", "image": url,
                                      "mimeType": part.get("mimeType", "image/png")})
            if items:
                wire.append({"role": "user", "content": items})
            continue
        if role == "assistant":
            items = []
            assistant_content = content
            if isinstance(assistant_content, str):
                assistant_content = [{"type": "text", "text": assistant_content}]
            for part in assistant_content or []:
                if isinstance(part, str):
                    items.append({"type": "text", "text": part})
                elif isinstance(part, dict):
                    if part.get("type") == "text":
                        items.append({"type": "text", "text": part.get("text", "")})
                    elif part.get("type") in ("tool_call", "tool-call"):
                        name = part.get("name") or (part.get("function") or {}).get("name", "")
                        args = part.get("arguments") or part.get("input")
                        if isinstance(args, dict):
                            args = json.dumps(args)
                        tool_name_map[part.get("id", "")] = name
                        items.append({"type": "tool-call", "toolCallId": part.get("id", ""),
                                      "toolName": name, "input": args})
                    elif part.get("type") in ("reasoning", "thinking"):
                        items.append({"type": "reasoning",
                                      "text": part.get("text") or part.get("thinking", "")})
            for call in msg.get("tool_calls") or []:
                function = call.get("function") or {}
                args = function.get("arguments", "")
                try:
                    args = json.loads(args) if isinstance(args, str) else args
                except json.JSONDecodeError:
                    pass
                tool_id = call.get("id", "")
                tool_name = function.get("name", "")
                tool_name_map[tool_id] = tool_name
                items.append({"type": "tool-call", "toolCallId": tool_id,
                              "toolName": tool_name, "input": args})
            if items:
                wire.append({"role": "assistant", "content": items})
            continue
        if role == "tool":
            items = []
            tool_content = content
            if isinstance(tool_content, str):
                tool_content = [tool_content]
            for part in tool_content or []:
                if isinstance(part, dict) and part.get("type") == "tool_result":
                    items.append(part)
                else:
                    items.append({
                        "type": "tool-result",
                        "toolCallId": msg.get("tool_call_id", ""),
                        "toolName": tool_name_map.get(msg.get("tool_call_id", ""), "unknown"),
                        "output": {"type": "text", "value": str(part if part is not None else "")},
                    })
            if items:
                wire.append({"role": "tool", "content": items})
            continue
        wire.append({"role": "user", "content": [{"type": "text", "text": str(content)}]})

    return wire
