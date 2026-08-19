"""
command-code-proxy — local OpenAI-compatible proxy for Command Code's HTTP API.

Translates OpenAI /v1/chat/completions <-> Command Code /alpha/generate so
any OpenAI-compatible client (OpenCode, LiteLLM, curl, etc.) can use a
Command Code subscription as a native model provider.

Endpoints:
  GET  /v1/models             -> model list (all Command Code models)
  GET  /health                -> health check
  POST /v1/chat/completions   -> non-stream (JSON) or stream (SSE)

Env:
  COMMAND_CODE_PROXY_PORT        listen port (default 18080)
  COMMAND_CODE_API_BASE          upstream base (default https://api.commandcode.ai)
  COMMAND_CODE_PROXY_HOST        bind address (default 127.0.0.1)
  COMMAND_CODE_PROXY_CORS        CORS origin (default unset = no CORS headers)
  COMMAND_CODE_PROXY_TIMEOUT     upstream timeout in seconds (default 600)
  COMMAND_CODE_PROXY_RETRIES     retry count for transient failures (default 2)
  COMMAND_CODE_PROXY_MAX_REQS    max concurrent requests (default 0 = unlimited)
  COMMAND_CODE_PROXY_MODELS      comma-separated allowlist of model IDs (default: all)
  COMMAND_CODE_PROXY_DEFAULT     default model ID (default: xiaomi/mimo-v2.5)
  COMMAND_CODE_PROXY_LOG_LEVEL   logging level (default: INFO)
"""

import errno
import json
import logging
import os
import signal
import sys
import threading
import time
import uuid
from typing import Any, Generator, Optional, Union

import http.client
import http.server

from .wire_format import (
    API_BASE,
    DEFAULT_MODEL,
    REASONING_EFFORTS,
    build_auth_headers,
    ensure_cli_updated_background,
    get_cli_version,
    get_git_info,
    get_model_catalog,
    load_config,
    parse_model_and_effort,
    wire_messages,
    wire_tools,
)
from .auth import get_auth_state

PORT = int(os.environ.get("COMMAND_CODE_PROXY_PORT", "18080"))
HOST = os.environ.get("COMMAND_CODE_PROXY_HOST", "127.0.0.1")
CORS_ORIGIN = os.environ.get("COMMAND_CODE_PROXY_CORS", "")
UPSTREAM_TIMEOUT = int(os.environ.get("COMMAND_CODE_PROXY_TIMEOUT", "600"))
MAX_RETRIES = int(os.environ.get("COMMAND_CODE_PROXY_RETRIES", "2"))
RETRY_BACKOFF = [0.5, 1.5]  # seconds between retries
MAX_CONCURRENT = int(os.environ.get("COMMAND_CODE_PROXY_MAX_REQS", "0"))
PROXY_DEFAULT_MODEL = os.environ.get("COMMAND_CODE_PROXY_DEFAULT", DEFAULT_MODEL)

log = logging.getLogger("command-code-proxy")

# Model allowlist: None = all models allowed, set = only these models
_allowed_models: Optional[set[str]] = None
_allowed_models_raw = os.environ.get("COMMAND_CODE_PROXY_MODELS", "")
if _allowed_models_raw.strip():
    _allowed_models = {m.strip() for m in _allowed_models_raw.split(",") if m.strip()}

# Concurrency semaphore: 0 = unlimited
_semaphore: Optional[threading.Semaphore] = None
if MAX_CONCURRENT > 0:
    _semaphore = threading.Semaphore(MAX_CONCURRENT)

# Suppress noisy tracebacks for benign client disconnects
_ERRNOS_SUPPRESSED = {errno.ECONNRESET, errno.EPIPE, errno.ECONNABORTED}


def _default_model() -> str:
    cfg = load_config()
    return cfg.get("model", PROXY_DEFAULT_MODEL)


def _is_model_allowed(model_id: str) -> bool:
    if _allowed_models is None:
        return True
    return model_id in _allowed_models


# --- Upstream call ---------------------------------------------------------


class UpstreamError(Exception):
    def __init__(self, status: int, error: dict):
        super().__init__(error.get("message", "upstream error"))
        self.status = status
        self.error = error


class UpstreamUnavailableError(Exception):
    """Raised on connection failures that may be transient."""
    pass


def _parse_api_base(base: str):
    """Parse API_BASE into (scheme, host, port)."""
    scheme, rest = base.split("://", 1)
    host, port = rest, (443 if scheme == "https" else 80)
    if ":" in host:
        host, port = host.rsplit(":", 1)
        port = int(port)
    return scheme, host, port


def _upstream_stream(model: str, messages: list, tools: list, system,
                     max_tokens: int, temperature, cwd: str,
                     reasoning_effort: Optional[str] = None,
                     timeout: Optional[int] = None) -> http.client.HTTPResponse:
    """POST the CLI-exact wire body to /alpha/generate and return the open
    HTTPResponse (streaming NDJSON)."""
    body: dict[str, Any] = {
        "config": get_git_info(cwd),
        "memory": None,
        "taste": None,
        "skills": None,
        "permissionMode": "standard",
        "mode": "agent",
        "params": {
            "model": model,
            "messages": messages,
            "tools": tools,
            "max_tokens": max_tokens,
            "stream": True,
        },
    }
    if system:
        body["params"]["system"] = system
    if temperature is not None:
        body["params"]["temperature"] = temperature
    if reasoning_effort:
        body["params"]["reasoning_effort"] = reasoning_effort

    headers = build_auth_headers(cwd)
    scheme, host, port = _parse_api_base(API_BASE)
    conn_cls = http.client.HTTPSConnection if scheme == "https" else http.client.HTTPConnection
    conn = conn_cls(host, port, timeout=timeout or UPSTREAM_TIMEOUT)
    try:
        conn.request("POST", "/alpha/generate", body=json.dumps(body), headers=headers)
        return conn.getresponse()
    except (ConnectionRefusedError, ConnectionResetError, OSError, TimeoutError) as e:
        conn.close()
        raise UpstreamUnavailableError(f"Connection to upstream failed: {e}") from e


# --- OpenAI completion assembly -------------------------------------------


def _completion_json(model: str, text: str, reasoning: str, tool_calls: list,
                     finish_reason: str, usage: dict) -> dict:
    msg: dict[str, Any] = {"role": "assistant", "content": text or None}
    if reasoning:
        msg["reasoning_content"] = reasoning
    if tool_calls:
        msg["tool_calls"] = [{
            "id": tc.get("toolCallId", ""),
            "type": "function",
            "function": {
                "name": tc.get("toolName", ""),
                "arguments": json.dumps(tc.get("input", {})),
            },
        } for tc in tool_calls]
    return {
        "id": f"chatcmpl-{uuid.uuid4().hex[:24]}",
        "object": "chat.completion",
        "created": int(time.time()),
        "model": model,
        "choices": [{
            "index": 0,
            "message": msg,
            "finish_reason": finish_reason,
        }],
        "usage": {
            "prompt_tokens": usage.get("inputTokens", 0),
            "completion_tokens": usage.get("outputTokens", 0),
            "total_tokens": usage.get("inputTokens", 0) + usage.get("outputTokens", 0),
            "prompt_tokens_details": {
                "cached_tokens": usage.get("cacheReadTokens", 0),
            },
        },
    }


def _map_finish_reason(raw: Optional[str]) -> str:
    r = (raw or "").lower()
    if r in ("tool_use", "tool-calls", "tool_calls"):
        return "tool_calls"
    if r in ("length", "max_tokens"):
        return "length"
    return "stop"


def _sse_line(obj: dict) -> str:
    return f"data: {json.dumps(obj)}\n\n"


def _translate_stream(resp, model: str, include_usage: bool) -> Generator[str, None, None]:
    """Translate the upstream NDJSON stream into OpenAI SSE chunks."""
    created = int(time.time())
    cid = f"chatcmpl-{uuid.uuid4().hex[:24]}"
    tool_index = 0
    tool_emitted: set[str] = set()
    text: list[str] = []
    reasoning: list[str] = []
    tool_calls: list[dict] = []
    usage: dict = {}
    raw_finish = None
    sent_header = False

    def chunk(delta: dict, finish_reason=None):
        return {
            "id": cid,
            "object": "chat.completion.chunk",
            "created": created,
            "model": model,
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish_reason}],
        }

    for raw in resp:
        if not raw:
            continue
        line = raw.decode("utf-8", errors="replace").strip()
        if not line:
            continue
        try:
            evt = json.loads(line)
        except json.JSONDecodeError:
            continue

        if not sent_header:
            sent_header = True
            yield _sse_line(chunk({"role": "assistant"}))

        etype = evt.get("type")
        if etype == "text-delta":
            t = evt.get("text", "")
            text.append(t)
            yield _sse_line(chunk({"content": t}))
        elif etype == "reasoning-delta":
            t = evt.get("text", "")
            reasoning.append(t)
            yield _sse_line(chunk({"reasoning_content": t}))
        elif etype == "tool-call":
            name = evt.get("toolName", "")
            args = evt.get("input", evt.get("args"))
            if isinstance(args, dict):
                args = json.dumps(args)
            else:
                args = str(args) if args is not None else ""
            tc_id = evt.get("toolCallId", f"call_{tool_index}")
            if tc_id not in tool_emitted:
                tool_emitted.add(tc_id)
                tool_calls.append({"toolCallId": tc_id, "toolName": name, "input": args})
                yield _sse_line(chunk({"tool_calls": [{
                    "index": tool_index,
                    "id": tc_id,
                    "type": "function",
                    "function": {"name": name, "arguments": ""},
                }]}))
                tool_index += 1
            yield _sse_line(chunk({"tool_calls": [{
                "index": tool_index - 1,
                "id": tc_id,
                "type": "function",
                "function": {"arguments": args},
            }]}))
        elif etype == "finish":
            total = evt.get("totalUsage") or {}
            details = total.get("inputTokenDetails") or {}
            usage = {
                "inputTokens": total.get("inputTokens", 0),
                "outputTokens": total.get("outputTokens", 0),
                "cacheReadTokens": details.get("cacheReadTokens", 0),
                "cacheWriteTokens": details.get("cacheWriteTokens", 0),
            }
            raw_finish = evt.get("rawFinishReason") or evt.get("finishReason")
            yield _sse_line(chunk({}, finish_reason=_map_finish_reason(raw_finish)))
            if include_usage:
                yield _sse_line({
                    "id": cid, "object": "chat.completion.chunk", "created": created,
                    "model": model, "choices": [],
                    "usage": {
                        "prompt_tokens": usage.get("inputTokens", 0),
                        "completion_tokens": usage.get("outputTokens", 0),
                        "total_tokens": usage.get("inputTokens", 0) + usage.get("outputTokens", 0),
                    },
                })
        elif etype == "error":
            err = evt.get("error")
            msg = err.get("message", "Stream error") if isinstance(err, dict) else str(err)
            yield _sse_line({"error": {"message": msg, "type": "command_code_error"}})
    yield "data: [DONE]\n\n"


# --- Chat handler with retry -----------------------------------------------


def _is_retryable(status: int) -> bool:
    return status in (502, 503, 504)


def _handle_chat(body: dict, request_timeout: Optional[int] = None) -> Union[list, Generator[str, None, None]]:
    raw_model = body.get("model") or _default_model()

    # Parse model:effort syntax (e.g. "claude-sonnet-5:high")
    model, effort_from_model = parse_model_and_effort(raw_model)

    # reasoning_effort: explicit body param takes precedence, then model:effort
    reasoning_effort = body.get("reasoning_effort") or effort_from_model
    if reasoning_effort and reasoning_effort not in REASONING_EFFORTS:
        reasoning_effort = None

    if not _is_model_allowed(model):
        raise UpstreamError(400, {
            "message": f"Model '{model}' is not in the allowed models list. "
                       f"Available: {', '.join(sorted(_allowed_models or set()))}",
            "type": "invalid_model",
        })

    raw_messages = body.get("messages") or []
    system = None
    filtered: list = []
    for msg in raw_messages:
        if msg.get("role") == "system":
            if system is None:
                sys_content = msg.get("content")
                if isinstance(sys_content, list):
                    system = " ".join(
                        p.get("text", "") if isinstance(p, dict) else str(p)
                        for p in sys_content
                    ).strip()
                else:
                    system = sys_content
            continue
        filtered.append(msg)
    messages = wire_messages(filtered)
    tools = wire_tools(body.get("tools") or [])
    max_tokens = body.get("max_tokens") or 64000
    temperature = body.get("temperature")
    stream = bool(body.get("stream"))
    include_usage = bool((body.get("stream_options") or {}).get("include_usage"))
    cwd = os.getcwd()

    last_error: Optional[Exception] = None
    for attempt in range(1 + MAX_RETRIES):
        try:
            resp = _upstream_stream(model, messages, tools, system, max_tokens,
                                    temperature, cwd, reasoning_effort=reasoning_effort,
                                    timeout=request_timeout)

            if resp.status != 200:
                err_raw = resp.read().decode("utf-8", errors="replace")
                try:
                    err = json.loads(err_raw)
                    if isinstance(err, dict) and "error" in err:
                        err = err["error"]
                except json.JSONDecodeError:
                    err = {"message": err_raw[:500]}
                if _is_retryable(resp.status) and attempt < MAX_RETRIES:
                    backoff = RETRY_BACKOFF[min(attempt, len(RETRY_BACKOFF) - 1)]
                    log.warning("upstream %d on attempt %d, retrying in %.1fs",
                                resp.status, attempt + 1, backoff)
                    time.sleep(backoff)
                    continue
                raise UpstreamError(resp.status, err)

            if stream:
                return _translate_stream(resp, model, include_usage)
            else:
                return _collect_non_stream(resp, model)

        except UpstreamUnavailableError as e:
            last_error = e
            if attempt < MAX_RETRIES:
                backoff = RETRY_BACKOFF[min(attempt, len(RETRY_BACKOFF) - 1)]
                log.warning("upstream unavailable on attempt %d: %s, retrying in %.1fs",
                            attempt + 1, e, backoff)
                time.sleep(backoff)
                continue
            raise UpstreamError(502, {"message": str(e),
                                      "type": "command_code_upstream_unavailable"}) from e
        except UpstreamError:
            raise
        except Exception as e:
            raise UpstreamError(502, {"message": f"Proxy error: {e}",
                                      "type": "command_code_proxy_error"}) from e

    raise UpstreamError(502, {"message": "Max retries exceeded",
                              "type": "command_code_max_retries"})


def _collect_non_stream(resp, model: str) -> list:
    """Collect a non-streaming NDJSON response into a single completion."""
    text: list[str] = []
    reasoning: list[str] = []
    tool_calls: list[dict] = []
    usage: dict = {}
    raw_finish = None
    for raw in resp:
        if not raw:
            continue
        try:
            evt = json.loads(raw.decode("utf-8", errors="replace"))
        except json.JSONDecodeError:
            continue
        etype = evt.get("type")
        if etype == "text-delta":
            text.append(evt.get("text", ""))
        elif etype == "reasoning-delta":
            reasoning.append(evt.get("text", ""))
        elif etype == "tool-call":
            args = evt.get("input", evt.get("args"))
            tool_calls.append({
                "toolCallId": evt.get("toolCallId", ""),
                "toolName": evt.get("toolName", ""),
                "input": args,
            })
        elif etype == "finish":
            total = evt.get("totalUsage") or {}
            details = total.get("inputTokenDetails") or {}
            usage = {
                "inputTokens": total.get("inputTokens", 0),
                "outputTokens": total.get("outputTokens", 0),
                "cacheReadTokens": details.get("cacheReadTokens", 0),
                "cacheWriteTokens": details.get("cacheWriteTokens", 0),
            }
            raw_finish = evt.get("rawFinishReason") or evt.get("finishReason")
        elif etype == "error":
            err = evt.get("error")
            msg = err.get("message", "Stream error") if isinstance(err, dict) else str(err)
            raise UpstreamError(502, {"message": msg, "type": "command_code_stream_error"})
    completion = _completion_json(
        model, "".join(text), "".join(reasoning), tool_calls,
        _map_finish_reason(raw_finish), usage,
    )
    return [completion]


# --- HTTP server -----------------------------------------------------------


class ProxyHandler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    server_version = "CommandCodeProxy/2.0"

    def log_message(self, fmt, *args):
        log.info("%s %s", self.address_string(), fmt % args)

    def handle_error(self, request, client_address):
        exc = sys.exc_info()[1]
        if isinstance(exc, OSError) and exc.errno in _ERRNOS_SUPPRESSED:
            return
        log.error("handle_error: %s", exc, exc_info=True)

    def _cors_headers(self):
        if CORS_ORIGIN:
            self.send_header("Access-Control-Allow-Origin", CORS_ORIGIN)
            self.send_header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
            self.send_header("Access-Control-Allow-Headers", "Content-Type, Authorization")
            self.send_header("Access-Control-Max-Age", "86400")

    def _send_json(self, status: int, obj: dict):
        payload = json.dumps(obj).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self._cors_headers()
        self.end_headers()
        self.wfile.write(payload)

    def _read_body(self) -> dict:
        length = int(self.headers.get("Content-Length", "0") or "0")
        if length <= 0:
            return {}
        try:
            return json.loads(self.rfile.read(length))
        except json.JSONDecodeError:
            raise UpstreamError(400, {"message": "Invalid JSON body",
                                      "type": "invalid_request_error"})

    def do_OPTIONS(self):
        self.send_response(204)
        self._cors_headers()
        self.end_headers()

    def do_GET(self):
        path = self.path.rstrip("/")
        if path in ("/v1/models", "/models"):
            default = _default_model()
            catalog = get_model_catalog()
            models_data = []
            for mid, meta in catalog.items():
                models_data.append({
                    "id": mid,
                    "object": "model",
                    "created": 0,
                    "owned_by": meta.get("provider", "command-code"),
                    "name": meta.get("name", mid),
                    "reasoning": meta.get("reasoning", False),
                    "efforts": meta.get("efforts", []),
                    "context_window": meta.get("context_window", 0),
                })
            self._send_json(200, {
                "object": "list",
                "data": models_data,
            })
            return
        if path == "/health":
            auth_health = get_auth_state().health_check()
            self._send_json(200, {
                "status": "ok",
                "version": get_cli_version(),
                "upstream": API_BASE,
                "models": len(get_model_catalog()),
                "default_model": PROXY_DEFAULT_MODEL,
                "auth": auth_health,
            })
            return
        self._send_json(404, {"error": {"message": f"Unknown route {self.path}",
                                        "type": "not_found"}})

    def do_POST(self):
        path = self.path.rstrip("/")
        if path not in ("/v1/chat/completions", "/chat/completions"):
            self._send_json(404, {"error": {"message": f"Unknown route {self.path}",
                                            "type": "not_found"}})
            return
        request_id = uuid.uuid4().hex[:12]
        t0 = time.monotonic()

        # Concurrency limiting
        if _semaphore is not None:
            if not _semaphore.acquire(blocking=False):
                log.warning("[%s] concurrency limit reached (%d)", request_id, MAX_CONCURRENT)
                self._send_json(429, {"error": {
                    "message": f"Concurrency limit reached ({MAX_CONCURRENT} max)",
                    "type": "rate_limit_error",
                }})
                return

        try:
            body = self._read_body()
        except UpstreamError as e:
            if _semaphore is not None:
                _semaphore.release()
            self._send_json(e.status, {"error": e.error})
            return

        # Allow per-request timeout override via header
        req_timeout = self.headers.get("X-Proxy-Timeout")
        request_timeout = int(req_timeout) if req_timeout else None

        try:
            result = _handle_chat(body, request_timeout=request_timeout)
        except UpstreamError as e:
            elapsed = time.monotonic() - t0
            log.error("[%s] upstream error %d in %.2fs: %s",
                      request_id, e.status, elapsed, e.error.get("message", ""))
            if _semaphore is not None:
                _semaphore.release()
            self._send_json(e.status, {"error": e.error})
            return
        except Exception as e:
            elapsed = time.monotonic() - t0
            log.error("[%s] proxy error in %.2fs: %s", request_id, elapsed, e)
            if _semaphore is not None:
                _semaphore.release()
            self._send_json(502, {"error": {"message": f"Proxy error: {e}",
                                            "type": "command_code_proxy_error"}})
            return

        if isinstance(result, list):
            elapsed = time.monotonic() - t0
            log.info("[%s] completed in %.2fs (non-stream)", request_id, elapsed)
            if _semaphore is not None:
                _semaphore.release()
            self._send_json(200, result[0])
            return

        # Streaming response — semaphore released after stream ends
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self._cors_headers()
        self.end_headers()
        chunks_sent = 0
        try:
            for sse in result:
                self.wfile.write(sse.encode("utf-8"))
                self.wfile.flush()
                chunks_sent += 1
        except (OSError, GeneratorExit):
            pass
        finally:
            if _semaphore is not None:
                _semaphore.release()
        elapsed = time.monotonic() - t0
        log.info("[%s] completed in %.2fs (stream, %d chunks)", request_id, elapsed, chunks_sent)


def _setup_signals(server):
    """Register signal handlers for graceful shutdown."""
    def shutdown_handler(signum, frame):
        log.info("received signal %d, shutting down", signum)
        threading.Thread(target=server.shutdown, daemon=True).start()

    signal.signal(signal.SIGTERM, shutdown_handler)
    signal.signal(signal.SIGINT, shutdown_handler)


def main():
    log_level = os.environ.get("COMMAND_CODE_PROXY_LOG_LEVEL", "INFO").upper()
    logging.basicConfig(
        level=getattr(logging, log_level, logging.INFO),
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
        stream=sys.stderr,
    )

    ensure_cli_updated_background()

    server = http.server.ThreadingHTTPServer((HOST, PORT), ProxyHandler)
    server.daemon_threads = True
    _setup_signals(server)

    log.info("listening on http://%s:%s (upstream: %s, timeout: %ds, retries: %d, models: %d)",
             HOST, PORT, API_BASE, UPSTREAM_TIMEOUT, MAX_RETRIES, len(get_model_catalog()))
    if _allowed_models:
        log.info("model allowlist: %s", ", ".join(sorted(_allowed_models)))
    if MAX_CONCURRENT > 0:
        log.info("concurrency limit: %d", MAX_CONCURRENT)
    if CORS_ORIGIN:
        log.info("CORS origin: %s", CORS_ORIGIN)

    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
        log.info("shutdown complete")


if __name__ == "__main__":
    main()
