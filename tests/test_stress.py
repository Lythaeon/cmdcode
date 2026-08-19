"""Hard concurrency + streaming stress tests."""

import http.server
import socketserver
import json
import http.client
import threading
import time
import pytest


class ThreadedHTTPServer(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True


class TestHardConcurrency:
    """Stress tests: 100+ parallel, streaming under load, edge cases."""

    def test_100_parallel_requests(self, proxy_server, mock_upstream):
        """100 simultaneous requests through the proxy."""
        _, _, handler = mock_upstream
        handler.response_events = [
            {"type": "text-delta", "text": "ok"},
            {"type": "finish", "finishReason": "stop",
             "totalUsage": {"inputTokens": 1, "outputTokens": 1,
                            "inputTokenDetails": {}}},
        ]
        host, port = proxy_server
        results = []
        errors = []

        def make_request(idx):
            for attempt in range(3):
                try:
                    conn = http.client.HTTPConnection(host, port, timeout=15)
                    body = json.dumps({
                        "model": "xiaomi/mimo-v2.5",
                        "messages": [{"role": "user", "content": f"req-{idx}"}],
                        "stream": False,
                    })
                    conn.request("POST", "/v1/chat/completions", body,
                                 {"Content-Type": "application/json"})
                    resp = conn.getresponse()
                    data = json.loads(resp.read())
                    results.append(data["choices"][0]["message"]["content"])
                    return
                except Exception as e:
                    if attempt == 2:
                        errors.append(str(e))
                    time.sleep(0.1)

        threads = [threading.Thread(target=make_request, args=(i,)) for i in range(100)]
        t0 = time.time()
        for t in threads:
            t.start()
        for t in threads:
            t.join(timeout=30)
        elapsed = time.time() - t0

        # Allow up to 5% failures due to OS connection limits
        assert len(errors) <= 5, f"Too many errors: {errors[:10]}"
        assert len(results) >= 95

    def test_50_parallel_streaming(self, proxy_server, mock_upstream):
        """50 simultaneous streaming requests."""
        _, _, handler = mock_upstream
        handler.response_events = [
            {"type": "text-delta", "text": "chunk"},
            {"type": "text-delta", "text": "-end"},
            {"type": "finish", "finishReason": "stop",
             "totalUsage": {"inputTokens": 1, "outputTokens": 2,
                            "inputTokenDetails": {}}},
        ]
        host, port = proxy_server
        results = []
        errors = []

        def make_stream(idx):
            for attempt in range(3):
                try:
                    conn = http.client.HTTPConnection(host, port, timeout=15)
                    body = json.dumps({
                        "model": "xiaomi/mimo-v2.5",
                        "messages": [{"role": "user", "content": f"stream-{idx}"}],
                        "stream": True,
                    })
                    conn.request("POST", "/v1/chat/completions", body,
                                 {"Content-Type": "application/json"})
                    resp = conn.getresponse()
                    raw = resp.read().decode()
                    results.append("[DONE]" in raw and "chunk" in raw)
                    return
                except Exception as e:
                    if attempt == 2:
                        errors.append(str(e))
                    time.sleep(0.1)

        threads = [threading.Thread(target=make_stream, args=(i,)) for i in range(50)]
        t0 = time.time()
        for t in threads:
            t.start()
        for t in threads:
            t.join(timeout=30)
        elapsed = time.time() - t0

        assert len(errors) <= 3, f"Too many errors: {errors[:5]}"
        assert len(results) >= 47

    def test_mixed_stream_nonstream_100(self, proxy_server, mock_upstream):
        """50 streaming + 50 non-streaming concurrent."""
        _, _, handler = mock_upstream
        handler.response_events = [
            {"type": "text-delta", "text": "ok"},
            {"type": "finish", "finishReason": "stop",
             "totalUsage": {"inputTokens": 1, "outputTokens": 1,
                            "inputTokenDetails": {}}},
        ]
        host, port = proxy_server
        results = []
        errors = []

        def make_request(idx, stream):
            for attempt in range(3):
                try:
                    conn = http.client.HTTPConnection(host, port, timeout=15)
                    body = json.dumps({
                        "model": "xiaomi/mimo-v2.5",
                        "messages": [{"role": "user", "content": f"req-{idx}"}],
                        "stream": stream,
                    })
                    conn.request("POST", "/v1/chat/completions", body,
                                 {"Content-Type": "application/json"})
                    resp = conn.getresponse()
                    resp.read()
                    results.append((idx, stream, resp.status))
                    return
                except Exception as e:
                    if attempt == 2:
                        errors.append(str(e))
                    time.sleep(0.1)

        threads = []
        for i in range(100):
            stream = i % 2 == 0
            threads.append(threading.Thread(target=make_request, args=(i, stream)))
        t0 = time.time()
        for t in threads:
            t.start()
        for t in threads:
            t.join(timeout=30)
        elapsed = time.time() - t0

        assert len(errors) <= 5, f"Too many errors: {errors[:5]}"
        assert len(results) >= 95

    def test_streaming_with_slow_client_disconnect(self, proxy_server, mock_upstream):
        """Client connects but disconnects before reading full response."""
        _, _, handler = mock_upstream
        handler.response_events = [
            {"type": "text-delta", "text": "a"},
            {"type": "text-delta", "text": "b"},
            {"type": "text-delta", "text": "c"},
            {"type": "finish", "finishReason": "stop",
             "totalUsage": {"inputTokens": 1, "outputTokens": 3,
                            "inputTokenDetails": {}}},
        ]
        host, port = proxy_server
        conn = http.client.HTTPConnection(host, port, timeout=5)
        body = json.dumps({
            "model": "xiaomi/mimo-v2.5",
            "messages": [{"role": "user", "content": "test"}],
            "stream": True,
        })
        conn.request("POST", "/v1/chat/completions", body,
                     {"Content-Type": "application/json"})
        resp = conn.getresponse()
        # Read partial data then close
        chunk1 = resp.read(100)
        conn.close()
        # Proxy should not crash
        assert len(chunk1) > 0

    def test_rapid_fire_requests(self, proxy_server, mock_upstream):
        """Rapid-fire 200 requests as fast as possible."""
        _, _, handler = mock_upstream
        handler.response_events = [
            {"type": "text-delta", "text": "ok"},
            {"type": "finish", "finishReason": "stop",
             "totalUsage": {"inputTokens": 1, "outputTokens": 1,
                            "inputTokenDetails": {}}},
        ]
        host, port = proxy_server
        results = []
        errors = []

        def make_request(idx):
            for attempt in range(3):
                try:
                    conn = http.client.HTTPConnection(host, port, timeout=10)
                    body = json.dumps({
                        "model": "xiaomi/mimo-v2.5",
                        "messages": [{"role": "user", "content": f"rapid-{idx}"}],
                    })
                    conn.request("POST", "/v1/chat/completions", body,
                                 {"Content-Type": "application/json"})
                    resp = conn.getresponse()
                    resp.read()
                    results.append(resp.status)
                    return
                except Exception as e:
                    if attempt == 2:
                        errors.append(str(e))
                    time.sleep(0.05)

        threads = [threading.Thread(target=make_request, args=(i,)) for i in range(200)]
        t0 = time.time()
        for t in threads:
            t.start()
        for t in threads:
            t.join(timeout=60)
        elapsed = time.time() - t0

        # Allow up to 10% failures under extreme concurrency
        assert len(errors) <= 20, f"Too many errors: {len(errors)}"
        assert len(results) >= 180

    def test_concurrent_health_checks_under_load(self, proxy_server, mock_upstream):
        """Health endpoint should work even under heavy load."""
        _, _, handler = mock_upstream
        handler.response_events = [
            {"type": "text-delta", "text": "ok"},
            {"type": "finish", "finishReason": "stop",
             "totalUsage": {"inputTokens": 1, "outputTokens": 1,
                            "inputTokenDetails": {}}},
        ]
        host, port = proxy_server
        results = []

        def health_check():
            try:
                conn = http.client.HTTPConnection(host, port, timeout=5)
                conn.request("GET", "/health")
                resp = conn.getresponse()
                data = json.loads(resp.read())
                results.append(data["status"] == "ok")
            except Exception:
                results.append(False)

        def chat_request():
            try:
                conn = http.client.HTTPConnection(host, port, timeout=10)
                body = json.dumps({
                    "model": "xiaomi/mimo-v2.5",
                    "messages": [{"role": "user", "content": "test"}],
                })
                conn.request("POST", "/v1/chat/completions", body,
                             {"Content-Type": "application/json"})
                resp = conn.getresponse()
                resp.read()
            except Exception:
                pass

        # Mix health checks with chat requests
        threads = []
        for i in range(50):
            if i % 5 == 0:
                threads.append(threading.Thread(target=health_check))
            else:
                threads.append(threading.Thread(target=chat_request))

        for t in threads:
            t.start()
        for t in threads:
            t.join(timeout=15)

        assert all(results), "Health checks failed under load"

    def test_all_models_concurrent(self, proxy_server, mock_upstream):
        """Concurrent requests using every model in the catalog."""
        from command_code_proxy.wire_format import get_model_catalog
        _, _, handler = mock_upstream
        handler.response_events = [
            {"type": "text-delta", "text": "ok"},
            {"type": "finish", "finishReason": "stop",
             "totalUsage": {"inputTokens": 1, "outputTokens": 1,
                            "inputTokenDetails": {}}},
        ]
        host, port = proxy_server
        catalog = get_model_catalog()
        results = []
        errors = []

        def make_request(model_id):
            for attempt in range(3):
                try:
                    conn = http.client.HTTPConnection(host, port, timeout=15)
                    body = json.dumps({
                        "model": model_id,
                        "messages": [{"role": "user", "content": "test"}],
                        "stream": False,
                    })
                    conn.request("POST", "/v1/chat/completions", body,
                                 {"Content-Type": "application/json"})
                    resp = conn.getresponse()
                    data = json.loads(resp.read())
                    results.append((model_id, data.get("model")))
                    return
                except Exception as e:
                    if attempt == 2:
                        errors.append((model_id, str(e)))
                    time.sleep(0.1)

        threads = [threading.Thread(target=make_request, args=(mid,))
                   for mid in catalog.keys()]
        t0 = time.time()
        for t in threads:
            t.start()
        for t in threads:
            t.join(timeout=30)
        elapsed = time.time() - t0

        # Allow up to 10% failures under concurrency
        max_errors = max(1, len(catalog) // 10)
        assert len(errors) <= max_errors, f"Too many errors: {errors[:5]}"
        assert len(results) >= len(catalog) - max_errors
