"""Concurrency tests — multiple simultaneous requests through the proxy."""

import json
import http.client
import threading
import pytest

from tests.conftest import MockUpstreamHandler


class TestConcurrentRequests:
    def test_sequential_requests_same_connection(self, proxy_server, mock_upstream):
        """Two requests on the same connection should work."""
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
            "model": "xiaomi/mimo-v2.5",
            "messages": [{"role": "user", "content": "test"}],
            "stream": False,
        })
        for _ in range(3):
            conn.request("POST", "/v1/chat/completions", body,
                         {"Content-Type": "application/json"})
            resp = conn.getresponse()
            data = json.loads(resp.read())
            assert data["choices"][0]["message"]["content"] == "ok"

    def test_parallel_requests(self, proxy_server, mock_upstream):
        """Multiple threads making requests simultaneously."""
        _, _, handler = mock_upstream
        handler.response_events = [
            {"type": "text-delta", "text": "parallel-ok"},
            {"type": "finish", "finishReason": "stop",
             "totalUsage": {"inputTokens": 1, "outputTokens": 1,
                            "inputTokenDetails": {"cacheReadTokens": 0}}},
        ]
        host, port = proxy_server
        results = []
        errors = []

        def make_request(idx):
            try:
                conn = http.client.HTTPConnection(host, port, timeout=10)
                body = json.dumps({
                    "model": "xiaomi/mimo-v2.5",
                    "messages": [{"role": "user", "content": f"req-{idx}"}],
                    "stream": False,
                })
                conn.request("POST", "/v1/chat/completions", body,
                             {"Content-Type": "application/json"})
                resp = conn.getresponse()
                data = json.loads(resp.read())
                results.append((idx, data["choices"][0]["message"]["content"]))
            except Exception as e:
                errors.append((idx, str(e)))

        threads = [threading.Thread(target=make_request, args=(i,)) for i in range(10)]
        for t in threads:
            t.start()
        for t in threads:
            t.join(timeout=15)

        assert len(errors) == 0, f"Errors: {errors}"
        assert len(results) == 10
        for idx, content in results:
            assert content == "parallel-ok"

    def test_parallel_streaming_requests(self, proxy_server, mock_upstream):
        """Multiple streaming requests at the same time."""
        _, _, handler = mock_upstream
        handler.response_events = [
            {"type": "text-delta", "text": "chunk"},
            {"type": "finish", "finishReason": "stop",
             "totalUsage": {"inputTokens": 1, "outputTokens": 1,
                            "inputTokenDetails": {"cacheReadTokens": 0}}},
        ]
        host, port = proxy_server
        results = []
        errors = []

        def make_stream_request(idx):
            try:
                conn = http.client.HTTPConnection(host, port, timeout=10)
                body = json.dumps({
                    "model": "xiaomi/mimo-v2.5",
                    "messages": [{"role": "user", "content": f"stream-{idx}"}],
                    "stream": True,
                })
                conn.request("POST", "/v1/chat/completions", body,
                             {"Content-Type": "application/json"})
                resp = conn.getresponse()
                raw = resp.read().decode()
                results.append((idx, "[DONE]" in raw and "data:" in raw))
            except Exception as e:
                errors.append((idx, str(e)))

        threads = [threading.Thread(target=make_stream_request, args=(i,)) for i in range(5)]
        for t in threads:
            t.start()
        for t in threads:
            t.join(timeout=15)

        assert len(errors) == 0, f"Errors: {errors}"
        assert len(results) == 5
        for idx, ok in results:
            assert ok, f"Request {idx} did not get proper SSE response"

    def test_each_request_gets_separate_upstream_call(self, proxy_server, mock_upstream):
        """Verify each proxy request creates a separate upstream request."""
        _, _, handler = mock_upstream
        handler.response_events = [
            {"type": "text-delta", "text": "ok"},
            {"type": "finish", "finishReason": "stop",
             "totalUsage": {"inputTokens": 1, "outputTokens": 1,
                            "inputTokenDetails": {"cacheReadTokens": 0}}},
        ]
        host, port = proxy_server

        def make_request(idx):
            conn = http.client.HTTPConnection(host, port, timeout=10)
            body = json.dumps({
                "model": "xiaomi/mimo-v2.5",
                "messages": [{"role": "user", "content": f"req-{idx}"}],
                "stream": False,
            })
            conn.request("POST", "/v1/chat/completions", body,
                         {"Content-Type": "application/json"})
            resp = conn.getresponse()
            return json.loads(resp.read())

        threads = [threading.Thread(target=make_request, args=(i,)) for i in range(5)]
        for t in threads:
            t.start()
        for t in threads:
            t.join(timeout=15)

        assert len(handler.request_log) == 5

    def test_concurrent_with_tool_calls(self, proxy_server, mock_upstream):
        """Parallel requests with tool calls."""
        _, _, handler = mock_upstream
        handler.response_events = [
            {"type": "tool-call", "toolCallId": "tc_1", "toolName": "fn",
             "input": {"x": 1}},
            {"type": "finish", "finishReason": "tool_use",
             "totalUsage": {"inputTokens": 5, "outputTokens": 3,
                            "inputTokenDetails": {"cacheReadTokens": 0}}},
        ]
        host, port = proxy_server
        results = []

        def make_request(idx):
            conn = http.client.HTTPConnection(host, port, timeout=10)
            body = json.dumps({
                "model": "xiaomi/mimo-v2.5",
                "messages": [{"role": "user", "content": f"tool-{idx}"}],
                "stream": True,
            })
            conn.request("POST", "/v1/chat/completions", body,
                         {"Content-Type": "application/json"})
            resp = conn.getresponse()
            raw = resp.read().decode()
            results.append("tool_calls" in raw and "fn" in raw)

        threads = [threading.Thread(target=make_request, args=(i,)) for i in range(5)]
        for t in threads:
            t.start()
        for t in threads:
            t.join(timeout=15)

        assert all(results), "Some requests did not get tool calls"
        assert len(results) == 5

    def test_concurrent_mixed_stream_and_nonstream(self, proxy_server, mock_upstream):
        """Mix of streaming and non-streaming concurrent requests."""
        _, _, handler = mock_upstream
        handler.response_events = [
            {"type": "text-delta", "text": "ok"},
            {"type": "finish", "finishReason": "stop",
             "totalUsage": {"inputTokens": 1, "outputTokens": 1,
                            "inputTokenDetails": {}}},
        ]
        host, port = proxy_server
        results = []

        def make_request(idx, stream):
            conn = http.client.HTTPConnection(host, port, timeout=10)
            body = json.dumps({
                "model": "xiaomi/mimo-v2.5",
                "messages": [{"role": "user", "content": f"req-{idx}"}],
                "stream": stream,
            })
            conn.request("POST", "/v1/chat/completions", body,
                         {"Content-Type": "application/json"})
            resp = conn.getresponse()
            raw = resp.read().decode()
            results.append((idx, stream, resp.status))

        threads = []
        for i in range(5):
            t = threading.Thread(target=make_request, args=(i, i % 2 == 0))
            threads.append(t)
        for t in threads:
            t.start()
        for t in threads:
            t.join(timeout=15)

        assert len(results) == 5
        for idx, stream, status in results:
            assert status == 200

    def test_concurrent_requests_different_models(self, proxy_server, mock_upstream):
        """Concurrent requests using different model IDs."""
        _, _, handler = mock_upstream
        handler.response_events = [
            {"type": "text-delta", "text": "ok"},
            {"type": "finish", "finishReason": "stop",
             "totalUsage": {"inputTokens": 1, "outputTokens": 1,
                            "inputTokenDetails": {}}},
        ]
        host, port = proxy_server
        models = ["xiaomi/mimo-v2.5", "gpt-5.6-sol", "claude-sonnet-5", "deepseek/deepseek-v4-pro"]
        results = []

        def make_request(idx, model):
            conn = http.client.HTTPConnection(host, port, timeout=10)
            body = json.dumps({
                "model": model,
                "messages": [{"role": "user", "content": f"req-{idx}"}],
                "stream": False,
            })
            conn.request("POST", "/v1/chat/completions", body,
                         {"Content-Type": "application/json"})
            resp = conn.getresponse()
            data = json.loads(resp.read())
            results.append((idx, model, data.get("model")))

        threads = [threading.Thread(target=make_request, args=(i, m))
                   for i, m in enumerate(models)]
        for t in threads:
            t.start()
        for t in threads:
            t.join(timeout=15)

        assert len(results) == 4
        for idx, model, returned_model in results:
            assert returned_model == model
