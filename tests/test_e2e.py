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
        assert len(data["data"]) >= 47  # dynamic from CLI catalog

    def test_models_have_required_fields(self, proxy_server):
        host, port = proxy_server
        conn = http.client.HTTPConnection(host, port, timeout=5)
        conn.request("GET", "/v1/models")
        resp = conn.getresponse()
        data = json.loads(resp.read())
        for model in data["data"]:
            assert "id" in model
            assert "object" in model
            assert model["object"] == "model"
            assert "owned_by" in model
            assert "name" in model
            assert "reasoning" in model

    def test_models_contains_gpt56(self, proxy_server):
        host, port = proxy_server
        conn = http.client.HTTPConnection(host, port, timeout=5)
        conn.request("GET", "/v1/models")
        resp = conn.getresponse()
        data = json.loads(resp.read())
        ids = [m["id"] for m in data["data"]]
        assert "gpt-5.6-luna" in ids
        assert "gpt-5.6-sol" in ids
        assert "claude-sonnet-5" in ids

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
        assert data["models"] >= 47  # dynamic from CLI catalog
        assert "default_model" in data

    def test_404_unknown_route(self, proxy_server):
        host, port = proxy_server
        conn = http.client.HTTPConnection(host, port, timeout=5)
        conn.request("GET", "/unknown")
        resp = conn.getresponse()
        assert resp.status == 404

    def test_v1_alias(self, proxy_server):
        host, port = proxy_server
        conn = http.client.HTTPConnection(host, port, timeout=5)
        conn.request("GET", "/models")
        resp = conn.getresponse()
        assert resp.status == 200


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

    def test_stream_empty_content_chunks(self, proxy_server, mock_upstream):
        """Empty text-delta should not crash."""
        _, _, handler = mock_upstream
        handler.response_events = [
            {"type": "text-delta", "text": ""},
            {"type": "text-delta", "text": "real"},
            {"type": "finish", "finishReason": "stop",
             "totalUsage": {"inputTokens": 1, "outputTokens": 1,
                            "inputTokenDetails": {}}},
        ]
        host, port = proxy_server
        conn = http.client.HTTPConnection(host, port, timeout=10)
        body = json.dumps({
            "model": "gpt-5.6-luna",
            "messages": [{"role": "user", "content": "test"}],
            "stream": True,
        })
        conn.request("POST", "/v1/chat/completions", body,
                     {"Content-Type": "application/json"})
        resp = conn.getresponse()
        assert resp.status == 200
        raw = resp.read().decode()
        assert "[DONE]" in raw

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

    def test_tool_calls_non_stream(self, proxy_server, mock_upstream):
        _, _, handler = mock_upstream
        handler.response_events = [
            {"type": "tool-call", "toolCallId": "tc_1", "toolName": "fn",
             "input": {"x": 1}},
            {"type": "finish", "finishReason": "tool_use",
             "totalUsage": {"inputTokens": 5, "outputTokens": 3,
                            "inputTokenDetails": {}}},
        ]
        host, port = proxy_server
        conn = http.client.HTTPConnection(host, port, timeout=10)
        body = json.dumps({
            "model": "gpt-5.6-luna",
            "messages": [{"role": "user", "content": "test"}],
            "stream": False,
        })
        conn.request("POST", "/v1/chat/completions", body,
                     {"Content-Type": "application/json"})
        resp = conn.getresponse()
        assert resp.status == 200
        data = json.loads(resp.read())
        assert "tool_calls" in data["choices"][0]["message"]
        assert data["choices"][0]["message"]["tool_calls"][0]["function"]["name"] == "fn"

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

    def test_reasoning_non_stream(self, proxy_server, mock_upstream):
        _, _, handler = mock_upstream
        handler.response_events = [
            {"type": "reasoning-delta", "text": "reasoning..."},
            {"type": "text-delta", "text": "final answer"},
            {"type": "finish", "finishReason": "stop",
             "totalUsage": {"inputTokens": 5, "outputTokens": 3,
                            "inputTokenDetails": {}}},
        ]
        host, port = proxy_server
        conn = http.client.HTTPConnection(host, port, timeout=10)
        body = json.dumps({
            "model": "gpt-5.6-luna",
            "messages": [{"role": "user", "content": "Think"}],
            "stream": False,
        })
        conn.request("POST", "/v1/chat/completions", body,
                     {"Content-Type": "application/json"})
        resp = conn.getresponse()
        assert resp.status == 200
        data = json.loads(resp.read())
        assert data["choices"][0]["message"]["reasoning_content"] == "reasoning..."
        assert data["choices"][0]["message"]["content"] == "final answer"

    def test_upstream_error_returns_error(self, proxy_server, mock_upstream):
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

    def test_upstream_request_body_format(self, proxy_server, mock_upstream):
        """Verify the proxy sends the correct wire format to upstream."""
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

        assert len(handler.request_log) == 1
        upstream_body = handler.request_log[0]["body"]
        assert upstream_body["mode"] == "agent"
        assert upstream_body["params"]["model"] == "gpt-5.6-luna"
        assert upstream_body["params"]["max_tokens"] == 1000
        assert upstream_body["params"]["system"] == "Be helpful."
        msgs = upstream_body["params"]["messages"]
        assert msgs[0]["role"] == "user"
        assert msgs[0]["content"] == [{"type": "text", "text": "Hello"}]
        tools = upstream_body["params"]["tools"]
        assert tools[0]["name"] == "test_fn"

    def test_upstream_receives_auth_headers(self, proxy_server, mock_upstream):
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
            "model": "gpt-5.6-luna",
            "messages": [{"role": "user", "content": "test"}],
        })
        conn.request("POST", "/v1/chat/completions", body,
                     {"Content-Type": "application/json"})
        resp = conn.getresponse()
        resp.read()

        assert len(handler.request_log) == 1
        headers = handler.request_log[0]["headers"]
        assert "Authorization" in headers
        assert headers["Authorization"].startswith("Bearer ")
        assert headers["User-Agent"] == "cli"
        assert "x-command-code-version" in headers
        assert "x-session-id" in headers

    def test_multiple_system_messages_use_first(self, proxy_server, mock_upstream):
        """Only the first system message should be extracted."""
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
            "model": "gpt-5.6-luna",
            "messages": [
                {"role": "system", "content": "first"},
                {"role": "system", "content": "second"},
                {"role": "user", "content": "hi"},
            ],
        })
        conn.request("POST", "/v1/chat/completions", body,
                     {"Content-Type": "application/json"})
        resp = conn.getresponse()
        resp.read()

        upstream_body = handler.request_log[0]["body"]
        assert upstream_body["params"]["system"] == "first"
