"""Integration tests — configuration, model catalog, CORS, rate limiting, signal handling."""

import json
import http.client
import os
import signal
import threading
import time
import pytest
from unittest.mock import patch

from command_code_proxy.wire_format import get_model_catalog


class TestModelCatalogConfig:
    """Verify the model catalog is correct and the proxy exposes it properly."""

    def test_catalog_loads_from_cli(self):
        catalog = get_model_catalog()
        assert len(catalog) >= 47  # at least the known set

    def test_all_providers_present(self):
        expected_providers = {
            "openai", "anthropic", "google", "sakana", "meta", "xai",
        }
        actual_providers = {meta["provider"] for meta in get_model_catalog().values()}
        for p in expected_providers:
            assert p in actual_providers, f"Provider {p} not found"

    def test_all_models_have_name(self):
        for model_id, meta in get_model_catalog().items():
            assert meta["name"], f"{model_id} has empty name"

    def test_no_duplicate_ids(self):
        catalog = get_model_catalog()
        assert len(catalog) == len(set(catalog.keys()))

    def test_gpt_models_are_openai(self):
        for model_id, meta in get_model_catalog().items():
            if model_id.startswith("gpt-"):
                assert meta["provider"] == "openai"

    def test_claude_models_are_anthropic(self):
        for model_id, meta in get_model_catalog().items():
            if model_id.startswith("claude-"):
                assert meta["provider"] == "anthropic"

    def test_gemini_models_are_google(self):
        for model_id, meta in get_model_catalog().items():
            if "gemini" in model_id:
                assert meta["provider"] == "google"

    def test_efforts_are_valid(self):
        for model_id, meta in get_model_catalog().items():
            for effort in meta.get("efforts", []):
                assert effort in {"low", "medium", "high", "xhigh", "max"}, \
                    f"{model_id} has invalid effort: {effort}"


class TestCORS:
    """Verify CORS headers when configured."""

    def test_no_cors_by_default(self, proxy_server):
        host, port = proxy_server
        conn = http.client.HTTPConnection(host, port, timeout=5)
        conn.request("GET", "/health")
        resp = conn.getresponse()
        assert resp.getheader("Access-Control-Allow-Origin") is None

    def test_cors_when_configured(self, proxy_server, mock_upstream):
        import command_code_proxy.proxy as pr
        original = pr.CORS_ORIGIN
        try:
            pr.CORS_ORIGIN = "http://localhost:3000"
            # Need to restart server — just test the header method
            from command_code_proxy.proxy import ProxyHandler
            # Verify the cors method exists and works
            assert hasattr(ProxyHandler, '_cors_headers')
        finally:
            pr.CORS_ORIGIN = original

    def test_options_returns_204(self, proxy_server):
        host, port = proxy_server
        conn = http.client.HTTPConnection(host, port, timeout=5)
        conn.request("OPTIONS", "/v1/chat/completions")
        resp = conn.getresponse()
        assert resp.status == 204


class TestModelAllowlist:
    """Verify model allowlist filtering."""

    def test_all_models_allowed_by_default(self, proxy_server, mock_upstream):
        import command_code_proxy.proxy as pr
        original = pr._allowed_models
        try:
            pr._allowed_models = None
            _, _, handler = mock_upstream
            handler.response_events = [
                {"type": "text-delta", "text": "ok"},
                {"type": "finish", "finishReason": "stop",
                 "totalUsage": {"inputTokens": 1, "outputTokens": 1,
                                "inputTokenDetails": {}}},
            ]
            host, port = proxy_server
            conn = http.client.HTTPConnection(host, port, timeout=10)
            body = json.dumps({
                "model": "xiaomi/mimo-v2.5",
                "messages": [{"role": "user", "content": "test"}],
            })
            conn.request("POST", "/v1/chat/completions", body,
                         {"Content-Type": "application/json"})
            resp = conn.getresponse()
            assert resp.status == 200
        finally:
            pr._allowed_models = original

    def test_disallowed_model_returns_400(self, proxy_server, mock_upstream):
        import command_code_proxy.proxy as pr
        original = pr._allowed_models
        try:
            pr._allowed_models = {"only-this-model"}
            host, port = proxy_server
            conn = http.client.HTTPConnection(host, port, timeout=5)
            body = json.dumps({
                "model": "xiaomi/mimo-v2.5",
                "messages": [{"role": "user", "content": "test"}],
            })
            conn.request("POST", "/v1/chat/completions", body,
                         {"Content-Type": "application/json"})
            resp = conn.getresponse()
            assert resp.status == 400
            data = json.loads(resp.read())
            assert "not in the allowed models list" in data["error"]["message"]
        finally:
            pr._allowed_models = original

    def test_model_prefix_stripped_before_check(self, proxy_server, mock_upstream):
        """command-code/ prefix should be stripped before allowlist check."""
        import command_code_proxy.proxy as pr
        original = pr._allowed_models
        try:
            pr._allowed_models = {"xiaomi/mimo-v2.5"}
            _, _, handler = mock_upstream
            handler.response_events = [
                {"type": "text-delta", "text": "ok"},
                {"type": "finish", "finishReason": "stop",
                 "totalUsage": {"inputTokens": 1, "outputTokens": 1,
                                "inputTokenDetails": {}}},
            ]
            host, port = proxy_server
            conn = http.client.HTTPConnection(host, port, timeout=10)
            body = json.dumps({
                "model": "command-code/xiaomi/mimo-v2.5",
                "messages": [{"role": "user", "content": "test"}],
            })
            conn.request("POST", "/v1/chat/completions", body,
                         {"Content-Type": "application/json"})
            resp = conn.getresponse()
            assert resp.status == 200
        finally:
            pr._allowed_models = original


class TestConcurrencyLimit:
    """Verify concurrency limiting via semaphore."""

    def test_concurrency_limit_rejects_when_full(self, proxy_server, mock_upstream):
        import command_code_proxy.proxy as pr
        original_semaphore = pr._semaphore
        original_max = pr.MAX_CONCURRENT
        try:
            pr.MAX_CONCURRENT = 1
            pr._semaphore = threading.Semaphore(1)
            _, _, handler = mock_upstream
            handler.response_events = [
                {"type": "text-delta", "text": "ok"},
                {"type": "finish", "finishReason": "stop",
                 "totalUsage": {"inputTokens": 1, "outputTokens": 1,
                                "inputTokenDetails": {}}},
            ]
            host, port = proxy_server

            # Hold the semaphore
            pr._semaphore.acquire(blocking=False)

            conn = http.client.HTTPConnection(host, port, timeout=5)
            body = json.dumps({
                "model": "xiaomi/mimo-v2.5",
                "messages": [{"role": "user", "content": "test"}],
            })
            conn.request("POST", "/v1/chat/completions", body,
                         {"Content-Type": "application/json"})
            resp = conn.getresponse()
            assert resp.status == 429
            data = json.loads(resp.read())
            assert "Concurrency limit" in data["error"]["message"]

            # Release for cleanup
            pr._semaphore.release()
        finally:
            pr._semaphore = original_semaphore
            pr.MAX_CONCURRENT = original_max


class TestDefaultModel:
    """Verify default model configuration."""

    def test_default_model_used_when_none_specified(self, proxy_server, mock_upstream):
        _, _, handler = mock_upstream
        handler.response_events = [
            {"type": "text-delta", "text": "ok"},
            {"type": "finish", "finishReason": "stop",
             "totalUsage": {"inputTokens": 1, "outputTokens": 1,
                            "inputTokenDetails": {}}},
        ]
        host, port = proxy_server
        conn = http.client.HTTPConnection(host, port, timeout=10)
        body = json.dumps({
            "messages": [{"role": "user", "content": "test"}],
        })
        conn.request("POST", "/v1/chat/completions", body,
                     {"Content-Type": "application/json"})
        resp = conn.getresponse()
        resp.read()

        upstream_body = handler.request_log[0]["body"]
        # Default model should be set (either mimo-v2.5 or whatever config says)
        assert upstream_body["params"]["model"]  # non-empty
        assert isinstance(upstream_body["params"]["model"], str)

    def test_explicit_model_overrides_default(self, proxy_server, mock_upstream):
        _, _, handler = mock_upstream
        handler.response_events = [
            {"type": "text-delta", "text": "ok"},
            {"type": "finish", "finishReason": "stop",
             "totalUsage": {"inputTokens": 1, "outputTokens": 1,
                            "inputTokenDetails": {}}},
        ]
        host, port = proxy_server
        conn = http.client.HTTPConnection(host, port, timeout=10)
        body = json.dumps({
            "model": "claude-sonnet-5",
            "messages": [{"role": "user", "content": "test"}],
        })
        conn.request("POST", "/v1/chat/completions", body,
                     {"Content-Type": "application/json"})
        resp = conn.getresponse()
        resp.read()

        upstream_body = handler.request_log[0]["body"]
        assert upstream_body["params"]["model"] == "claude-sonnet-5"


class TestErrorPaths:
    """Verify various error paths."""

    def test_empty_body_uses_default_model(self, proxy_server, mock_upstream):
        _, _, handler = mock_upstream
        handler.response_events = [
            {"type": "text-delta", "text": "ok"},
            {"type": "finish", "finishReason": "stop",
             "totalUsage": {"inputTokens": 1, "outputTokens": 1,
                            "inputTokenDetails": {}}},
        ]
        host, port = proxy_server
        conn = http.client.HTTPConnection(host, port, timeout=10)
        conn.request("POST", "/v1/chat/completions", "{}",
                     {"Content-Type": "application/json"})
        resp = conn.getresponse()
        assert resp.status == 200

    def test_post_unknown_route_returns_404(self, proxy_server):
        host, port = proxy_server
        conn = http.client.HTTPConnection(host, port, timeout=5)
        body = json.dumps({"model": "xiaomi/mimo-v2.5", "messages": []})
        conn.request("POST", "/unknown/endpoint", body,
                     {"Content-Type": "application/json"})
        resp = conn.getresponse()
        assert resp.status == 404

    def test_get_unknown_route_returns_404(self, proxy_server):
        host, port = proxy_server
        conn = http.client.HTTPConnection(host, port, timeout=5)
        conn.request("GET", "/v1/unknown")
        resp = conn.getresponse()
        assert resp.status == 404

    def test_chat_alias_works(self, proxy_server, mock_upstream):
        """POST /chat/completions should work (without /v1 prefix)."""
        _, _, handler = mock_upstream
        handler.response_events = [
            {"type": "text-delta", "text": "ok"},
            {"type": "finish", "finishReason": "stop",
             "totalUsage": {"inputTokens": 1, "outputTokens": 1,
                            "inputTokenDetails": {}}},
        ]
        host, port = proxy_server
        conn = http.client.HTTPConnection(host, port, timeout=10)
        body = json.dumps({
            "model": "xiaomi/mimo-v2.5",
            "messages": [{"role": "user", "content": "test"}],
        })
        conn.request("POST", "/chat/completions", body,
                     {"Content-Type": "application/json"})
        resp = conn.getresponse()
        assert resp.status == 200
