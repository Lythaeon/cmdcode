"""E2E tests — full proxy round-trip with mock upstream."""

import json
import http.client
import threading
import pytest

from tests.conftest import MockUpstreamHandler


class TestModelsEndpoint:
    def test_models_returns_list(self, proxy_server):
        host, port = proxy_server
        conn = http.client.HTTPConnection(host, port, timeout=5)
        conn.request("GET", "/v1/models")
        resp = conn.getresponse()
        assert resp.status == 200
        data = json.loads(resp.read())
        assert data["object"] == "list"
        assert len(data["data"]) >= 1
        assert data["data"][0]["owned_by"] == "command-code"

    def test_health_endpoint(self, proxy_server):
        host, port = proxy_server
        conn = http.client.HTTPConnection(host, port, timeout=5)
        conn.request("GET", "/health")
        resp = conn.getresponse()
        assert resp.status == 200
        data = json.loads(resp.read())
        assert data["status"] == "ok"
        assert "version" in data
        assert "upstream" in data

    def test_404_unknown_route(self, proxy_server):
        host, port = proxy_server
        conn = http.client.HTTPConnection(host, port, timeout=5)
        conn.request("GET", "/unknown")
        resp = conn.getresponse()
        assert resp.status == 404


class TestChatCompletion:
    def test_non_stream_completion(self, proxy_server, mock_upstream):
        _, _, handler = mock_upstream
        handler.response_events = [
            {"type": "text-delta", "text": "Hi"},
            {"type": "text-delta", "text": " there"},
            {"type": "finish", "finishReason": "stop",
             "totalUsage": {"inputTokens": 5, "outputTokens": 2,
                            "inputTokenDetails": {"cacheReadTokens": 0}}},
        ]
        host, port = proxy_server
        conn = http.client.HTTPConnection(host, port, timeout=10)
        body = json.dumps({
            "model": "gpt-5.6-luna",
            "messages": [{"role": "user", "content": "Hello"}],
            "stream": False,
        })
        conn.request("POST", "/v1/chat/completions", body,
                     {"Content-Type": "application/json"})
        resp = conn.getresponse()
        assert resp.status == 200
        data = json.loads(resp.read())
        assert data["object"] == "chat.completion"
        assert data["choices"][0]["message"]["content"] == "Hi there"
        assert data["usage"]["prompt_tokens"] == 5

    def test_stream_completion(self, proxy_server, mock_upstream):
        _, _, handler = mock_upstream
        handler.response_events = [
            {"type": "text-delta", "text": "Streamed"},
            {"type": "text-delta", "text": " response"},
            {"type": "finish", "finishReason": "stop",
             "totalUsage": {"inputTokens": 3, "outputTokens": 2,
                            "inputTokenDetails": {"cacheReadTokens": 0}}},
        ]
        host, port = proxy_server
        conn = http.client.HTTPConnection(host, port, timeout=10)
        body = json.dumps({
            "model": "gpt-5.6-luna",
            "messages": [{"role": "user", "content": "Stream test"}],
            "stream": True,
        })
        conn.request("POST", "/v1/chat/completions", body,
                     {"Content-Type": "application/json"})
        resp = conn.getresponse()
        assert resp.status == 200
        assert resp.getheader("Content-Type") == "text/event-stream"
        raw = resp.read().decode()
        assert "data:" in raw
        assert "[DONE]" in raw
        lines = [l for l in raw.split("\n") if l.startswith("data: ") and l != "data: [DONE]"]
        assert len(lines) >= 3

    def test_tool_calls_stream(self, proxy_server, mock_upstream):
        _, _, handler = mock_upstream
        handler.response_events = [
            {"type": "tool-call", "toolCallId": "tc_1", "toolName": "search",
             "input": {"query": "test"}},
            {"type": "finish", "finishReason": "tool_use",
             "totalUsage": {"inputTokens": 10, "outputTokens": 5,
                            "inputTokenDetails": {"cacheReadTokens": 0}}},
        ]
        host, port = proxy_server
        conn = http.client.HTTPConnection(host, port, timeout=10)
        body = json.dumps({
            "model": "gpt-5.6-luna",
            "messages": [{"role": "user", "content": "Search test"}],
            "stream": True,
        })
        conn.request("POST", "/v1/chat/completions", body,
                     {"Content-Type": "application/json"})
        resp = conn.getresponse()
        assert resp.status == 200
        raw = resp.read().decode()
        assert "tool_calls" in raw
        assert "search" in raw

    def test_upstream_error_returns_error(self, proxy_server, mock_upstream):
        """Non-retryable upstream errors (500) are returned as-is."""
        _, _, handler = mock_upstream
        handler.response_status = 500
        host, port = proxy_server
        conn = http.client.HTTPConnection(host, port, timeout=10)
        body = json.dumps({
            "model": "gpt-5.6-luna",
            "messages": [{"role": "user", "content": "Error test"}],
        })
        conn.request("POST", "/v1/chat/completions", body,
                     {"Content-Type": "application/json"})
        resp = conn.getresponse()
        assert resp.status == 500
        data = json.loads(resp.read())
        assert "error" in data

    def test_retryable_upstream_error_returns_502(self, proxy_server, mock_upstream):
        """Retryable upstream errors (502) exhaust retries and return 502."""
        _, _, handler = mock_upstream
        handler.response_status = 502
        host, port = proxy_server
        conn = http.client.HTTPConnection(host, port, timeout=10)
        body = json.dumps({
            "model": "gpt-5.6-luna",
            "messages": [{"role": "user", "content": "Retry test"}],
        })
        conn.request("POST", "/v1/chat/completions", body,
                     {"Content-Type": "application/json"})
        resp = conn.getresponse()
        assert resp.status == 502

    def test_invalid_json_body(self, proxy_server):
        host, port = proxy_server
        conn = http.client.HTTPConnection(host, port, timeout=5)
        conn.request("POST", "/v1/chat/completions", "not json",
                     {"Content-Type": "application/json"})
        resp = conn.getresponse()
        assert resp.status == 400

    def test_reasoning_stream(self, proxy_server, mock_upstream):
        _, _, handler = mock_upstream
        handler.response_events = [
            {"type": "reasoning-delta", "text": "thinking..."},
            {"type": "text-delta", "text": "answer"},
            {"type": "finish", "finishReason": "stop",
             "totalUsage": {"inputTokens": 5, "outputTokens": 3,
                            "inputTokenDetails": {"cacheReadTokens": 0}}},
        ]
        host, port = proxy_server
        conn = http.client.HTTPConnection(host, port, timeout=10)
        body = json.dumps({
            "model": "gpt-5.6-luna",
            "messages": [{"role": "user", "content": "Think"}],
            "stream": True,
        })
        conn.request("POST", "/v1/chat/completions", body,
                     {"Content-Type": "application/json"})
        resp = conn.getresponse()
        assert resp.status == 200
        raw = resp.read().decode()
        assert "reasoning_content" in raw
        assert "thinking..." in raw

    def test_upstream_request_body_format(self, proxy_server, mock_upstream):
        """Verify the proxy sends the correct wire format to upstream."""
        _, _, handler = mock_upstream
        handler.response_events = [
            {"type": "text-delta", "text": "ok"},
            {"type": "finish", "finishReason": "stop",
             "totalUsage": {"inputTokens": 1, "outputTokens": 1,
                            "inputTokenDetails": {"cacheReadTokens": 0}}},
        ]
        host, port = proxy_server
        conn = http.client.HTTPConnection(host, port, timeout=10)
        body = json.dumps({
            "model": "gpt-5.6-luna",
            "messages": [
                {"role": "system", "content": "Be helpful."},
                {"role": "user", "content": "Hello"},
            ],
            "tools": [{
                "type": "function",
                "function": {"name": "test_fn", "description": "A test function"},
            }],
            "max_tokens": 1000,
            "stream": False,
        })
        conn.request("POST", "/v1/chat/completions", body,
                     {"Content-Type": "application/json"})
        resp = conn.getresponse()
        resp.read()

        # Check upstream received proper wire format
        assert len(handler.request_log) == 1
        upstream_body = handler.request_log[0]["body"]
        assert upstream_body["mode"] == "agent"
        assert upstream_body["params"]["model"] == "gpt-5.6-luna"
        assert upstream_body["params"]["max_tokens"] == 1000
        # System message should be extracted
        assert upstream_body["params"]["system"] == "Be helpful."
        # Messages should be in wire format
        msgs = upstream_body["params"]["messages"]
        assert msgs[0]["role"] == "user"
        assert msgs[0]["content"] == [{"type": "text", "text": "Hello"}]
        # Tools should be in wire format
        tools = upstream_body["params"]["tools"]
        assert tools[0]["name"] == "test_fn"
