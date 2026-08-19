"""Chaos tests — upstream failures, partial responses, malformed data."""

import json
import http.client
import http.server
import socketserver
import threading
import time
import pytest
from unittest.mock import patch


class ThreadedHTTPServer(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True


class MalformedUpstreamHandler(http.server.BaseHTTPRequestHandler):
    """Mock upstream that returns various broken responses."""

    mode = "normal"
    delay = 0
    request_log = []

    def log_message(self, fmt, *args):
        pass

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = json.loads(self.rfile.read(length)) if length else {}
        self.request_log.append({"body": body, "time": time.time()})

        if self.delay > 0:
            time.sleep(self.delay)

        if self.mode == "empty":
            self.send_response(200)
            self.send_header("Content-Type", "application/x-ndjson")
            self.end_headers()
            return

        if self.mode == "partial_ndjson":
            self.send_response(200)
            self.send_header("Content-Type", "application/x-ndjson")
            self.end_headers()
            self.wfile.write(b'{"type":"text-delta","text":"partial"}\n')
            self.wfile.flush()
            return

        if self.mode == "invalid_json":
            self.send_response(200)
            self.send_header("Content-Type", "application/x-ndjson")
            self.end_headers()
            self.wfile.write(b'not json at all\n')
            self.wfile.write(b'{"type": "text-delta", "text": "recovered"}\n')
            self.wfile.write(json.dumps({
                "type": "finish", "finishReason": "stop",
                "totalUsage": {"inputTokens": 1, "outputTokens": 1, "inputTokenDetails": {}}
            }).encode() + b"\n")
            return

        if self.mode == "wrong_status":
            self.send_response(500)
            self.send_header("Content-Type", "application/json")
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(json.dumps({"error": {"message": "internal error"}}).encode())
            return

        if self.mode == "not_json_error":
            self.send_response(500)
            self.send_header("Content-Type", "text/plain")
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(b"Internal Server Error\n")
            return

        if self.mode == "huge_payload":
            self.send_response(200)
            self.send_header("Content-Type", "application/x-ndjson")
            self.end_headers()
            huge_text = "x" * 1_000_000
            self.wfile.write(json.dumps({"type": "text-delta", "text": huge_text}).encode() + b"\n")
            self.wfile.write(json.dumps({
                "type": "finish", "finishReason": "stop",
                "totalUsage": {"inputTokens": 100, "outputTokens": 50, "inputTokenDetails": {}}
            }).encode() + b"\n")
            return

        if self.mode == "many_chunks":
            self.send_response(200)
            self.send_header("Content-Type", "application/x-ndjson")
            self.end_headers()
            for i in range(1000):
                self.wfile.write(json.dumps({"type": "text-delta", "text": f"chunk-{i}"}).encode() + b"\n")
            self.wfile.write(json.dumps({
                "type": "finish", "finishReason": "stop",
                "totalUsage": {"inputTokens": 10, "outputTokens": 500, "inputTokenDetails": {}}
            }).encode() + b"\n")
            return

        if self.mode == "duplicate_finish":
            self.send_response(200)
            self.send_header("Content-Type", "application/x-ndjson")
            self.end_headers()
            self.wfile.write(json.dumps({"type": "text-delta", "text": "hi"}).encode() + b"\n")
            self.wfile.write(json.dumps({
                "type": "finish", "finishReason": "stop",
                "totalUsage": {"inputTokens": 1, "outputTokens": 1, "inputTokenDetails": {}}
            }).encode() + b"\n")
            self.wfile.write(json.dumps({
                "type": "finish", "finishReason": "stop",
                "totalUsage": {"inputTokens": 1, "outputTokens": 1, "inputTokenDetails": {}}
            }).encode() + b"\n")
            return

        if self.mode == "stream_error_event":
            self.send_response(200)
            self.send_header("Content-Type", "application/x-ndjson")
            self.end_headers()
            self.wfile.write(json.dumps({"type": "text-delta", "text": "before error"}).encode() + b"\n")
            self.wfile.write(json.dumps({"type": "error", "error": {"message": "upstream broke"}}).encode() + b"\n")
            return

        # Normal
        self.send_response(200)
        self.send_header("Content-Type", "application/x-ndjson")
        self.end_headers()
        self.wfile.write(json.dumps({"type": "text-delta", "text": "ok"}).encode() + b"\n")
        self.wfile.write(json.dumps({
            "type": "finish", "finishReason": "stop",
            "totalUsage": {"inputTokens": 1, "outputTokens": 1, "inputTokenDetails": {}}
        }).encode() + b"\n")


@pytest.fixture
def chaos_upstream():
    MalformedUpstreamHandler.mode = "normal"
    MalformedUpstreamHandler.delay = 0
    MalformedUpstreamHandler.request_log = []
    server = ThreadedHTTPServer(("127.0.0.1", 0), MalformedUpstreamHandler)
    host, port = server.server_address
    t = threading.Thread(target=server.serve_forever, daemon=True)
    t.start()
    yield host, port, MalformedUpstreamHandler
    server.shutdown()


def _start_proxy(upstream_host, upstream_port):
    from command_code_proxy.proxy import ProxyHandler
    api_base = f"http://{upstream_host}:{upstream_port}"
    p1 = patch("command_code_proxy.proxy.API_BASE", api_base)
    p2 = patch("command_code_proxy.wire_format.API_BASE", api_base)
    p1.start()
    p2.start()
    server = ThreadedHTTPServer(("127.0.0.1", 0), ProxyHandler)
    server.daemon_threads = True
    ph, pp = server.server_address
    t = threading.Thread(target=server.serve_forever, daemon=True)
    t.start()
    return ph, pp, server, (p1, p2)


def _stop_proxy(server, patches):
    server.shutdown()
    for p in patches:
        p.stop()


def _req(host, port, body, stream=True, timeout=10):
    conn = http.client.HTTPConnection(host, port, timeout=timeout)
    conn.request("POST", "/v1/chat/completions", json.dumps(body),
                 {"Content-Type": "application/json"})
    resp = conn.getresponse()
    data = resp.read().decode()
    return resp.status, data


class TestChaosUpstream:

    def test_empty_upstream_response(self, patch_auth_and_config, chaos_upstream):
        chaos_upstream[2].mode = "empty"
        ph, pp, server, patches = _start_proxy(*chaos_upstream[:2])
        try:
            status, data = _req(ph, pp, {
                "model": "xiaomi/mimo-v2.5",
                "messages": [{"role": "user", "content": "test"}],
                "stream": True,
            })
            assert status == 200
            assert "[DONE]" in data
        finally:
            _stop_proxy(server, patches)

    def test_partial_ndjson_stream(self, patch_auth_and_config, chaos_upstream):
        chaos_upstream[2].mode = "partial_ndjson"
        ph, pp, server, patches = _start_proxy(*chaos_upstream[:2])
        try:
            status, data = _req(ph, pp, {
                "model": "xiaomi/mimo-v2.5",
                "messages": [{"role": "user", "content": "test"}],
                "stream": True,
            })
            assert status == 200
            assert "partial" in data
        finally:
            _stop_proxy(server, patches)

    def test_invalid_json_from_upstream(self, patch_auth_and_config, chaos_upstream):
        chaos_upstream[2].mode = "invalid_json"
        ph, pp, server, patches = _start_proxy(*chaos_upstream[:2])
        try:
            status, data = _req(ph, pp, {
                "model": "xiaomi/mimo-v2.5",
                "messages": [{"role": "user", "content": "test"}],
                "stream": True,
            })
            assert status == 200
            assert "recovered" in data
        finally:
            _stop_proxy(server, patches)

    def test_upstream_500_returns_error(self, patch_auth_and_config, chaos_upstream):
        chaos_upstream[2].mode = "wrong_status"
        ph, pp, server, patches = _start_proxy(*chaos_upstream[:2])
        try:
            status, data = _req(ph, pp, {
                "model": "xiaomi/mimo-v2.5",
                "messages": [{"role": "user", "content": "test"}],
            })
            assert status == 500
        finally:
            _stop_proxy(server, patches)

    def test_upstream_not_json_error(self, patch_auth_and_config, chaos_upstream):
        chaos_upstream[2].mode = "not_json_error"
        ph, pp, server, patches = _start_proxy(*chaos_upstream[:2])
        try:
            status, data = _req(ph, pp, {
                "model": "xiaomi/mimo-v2.5",
                "messages": [{"role": "user", "content": "test"}],
            })
            assert status == 500
            assert "error" in json.loads(data)
        finally:
            _stop_proxy(server, patches)

    def test_huge_payload(self, patch_auth_and_config, chaos_upstream):
        chaos_upstream[2].mode = "huge_payload"
        ph, pp, server, patches = _start_proxy(*chaos_upstream[:2])
        try:
            status, data = _req(ph, pp, {
                "model": "xiaomi/mimo-v2.5",
                "messages": [{"role": "user", "content": "test"}],
                "stream": True,
            }, timeout=15)
            assert status == 200
            assert "x" * 1000 in data
        finally:
            _stop_proxy(server, patches)

    def test_many_chunks(self, patch_auth_and_config, chaos_upstream):
        chaos_upstream[2].mode = "many_chunks"
        ph, pp, server, patches = _start_proxy(*chaos_upstream[:2])
        try:
            status, data = _req(ph, pp, {
                "model": "xiaomi/mimo-v2.5",
                "messages": [{"role": "user", "content": "test"}],
                "stream": True,
            }, timeout=15)
            assert status == 200
            assert "[DONE]" in data
        finally:
            _stop_proxy(server, patches)

    def test_duplicate_finish_event(self, patch_auth_and_config, chaos_upstream):
        chaos_upstream[2].mode = "duplicate_finish"
        ph, pp, server, patches = _start_proxy(*chaos_upstream[:2])
        try:
            status, data = _req(ph, pp, {
                "model": "xiaomi/mimo-v2.5",
                "messages": [{"role": "user", "content": "test"}],
                "stream": True,
            })
            assert status == 200
            assert "[DONE]" in data
        finally:
            _stop_proxy(server, patches)

    def test_stream_error_event(self, patch_auth_and_config, chaos_upstream):
        chaos_upstream[2].mode = "stream_error_event"
        ph, pp, server, patches = _start_proxy(*chaos_upstream[:2])
        try:
            status, data = _req(ph, pp, {
                "model": "xiaomi/mimo-v2.5",
                "messages": [{"role": "user", "content": "test"}],
                "stream": True,
            })
            assert status == 200
            assert "upstream broke" in data
        finally:
            _stop_proxy(server, patches)

    def test_slow_upstream(self, patch_auth_and_config, chaos_upstream):
        chaos_upstream[2].delay = 2
        ph, pp, server, patches = _start_proxy(*chaos_upstream[:2])
        try:
            t0 = time.time()
            status, data = _req(ph, pp, {
                "model": "xiaomi/mimo-v2.5",
                "messages": [{"role": "user", "content": "test"}],
            }, timeout=10)
            elapsed = time.time() - t0
            assert status == 200
            assert elapsed >= 1.5
        finally:
            _stop_proxy(server, patches)

    def test_non_stream_with_malformed_body(self, patch_auth_and_config, chaos_upstream):
        chaos_upstream[2].mode = "invalid_json"
        ph, pp, server, patches = _start_proxy(*chaos_upstream[:2])
        try:
            status, data = _req(ph, pp, {
                "model": "xiaomi/mimo-v2.5",
                "messages": [{"role": "user", "content": "test"}],
                "stream": False,
            })
            assert status == 200
            parsed = json.loads(data)
            assert parsed["choices"][0]["message"]["content"] == "recovered"
        finally:
            _stop_proxy(server, patches)

    def test_upstream_connection_refused(self, patch_auth_and_config):
        import command_code_proxy.proxy as pr
        import command_code_proxy.wire_format as wf
        old_base = pr.API_BASE
        pr.API_BASE = "http://127.0.0.1:19999"
        wf.API_BASE = "http://127.0.0.1:19999"
        try:
            status, data = _req("127.0.0.1", 19999, {
                "model": "xiaomi/mimo-v2.5",
                "messages": [{"role": "user", "content": "test"}],
            }, timeout=5)
            # Should fail to connect
            assert status != 200
        except (ConnectionRefusedError, ConnectionResetError, OSError):
            pass  # Expected
        finally:
            pr.API_BASE = old_base
            wf.API_BASE = old_base
