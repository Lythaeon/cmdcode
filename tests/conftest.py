"""Shared test fixtures for command-code-proxy tests."""

import json
import http.server
import socketserver
import threading
import time
from pathlib import Path
from unittest.mock import patch, MagicMock

import pytest


class ThreadedHTTPServer(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True


@pytest.fixture(autouse=True)
def patch_auth_and_config(tmp_path):
    """Auto-patch auth and config for all tests."""
    auth_dir = tmp_path / ".commandcode"
    auth_dir.mkdir()
    (auth_dir / "auth.json").write_text(json.dumps({
        "apiKey": "test-api-key-12345678",
        "userId": "user-123",
        "userName": "testuser",
    }))
    (auth_dir / "config.json").write_text(json.dumps({
        "model": "xiaomi/mimo-v2.5",
        "tasteLearning": True,
        "oauthEnforced": False,
    }))
    with patch("command_code_proxy.wire_format.AUTH_DIR", auth_dir), \
         patch("command_code_proxy.wire_format.load_config", return_value={"model": "xiaomi/mimo-v2.5"}), \
         patch("command_code_proxy.proxy.ensure_cli_updated_background"), \
         patch("command_code_proxy.wire_format.get_cli_version", return_value="1.2.3"):
        yield auth_dir


class MockUpstreamHandler(http.server.BaseHTTPRequestHandler):
    """Mock Command Code upstream that returns NDJSON streams."""

    response_events = []
    response_status = 200
    request_log = []

    def log_message(self, fmt, *args):
        pass

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = json.loads(self.rfile.read(length)) if length else {}
        self.request_log.append({
            "path": self.path,
            "headers": dict(self.headers),
            "body": body,
            "timestamp": time.time(),
        })

        if self.response_status != 200:
            self.send_response(self.response_status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(json.dumps({"error": {"message": "upstream error"}}).encode())
            return

        self.send_response(200)
        self.send_header("Content-Type", "application/x-ndjson")
        self.end_headers()

        for event in self.response_events:
            self.wfile.write((json.dumps(event) + "\n").encode())
            self.wfile.flush()


@pytest.fixture
def mock_upstream():
    """Start a mock upstream server."""
    MockUpstreamHandler.response_events = [
        {"type": "text-delta", "text": "Hello"},
        {"type": "text-delta", "text": " world"},
        {"type": "finish", "finishReason": "stop",
         "totalUsage": {"inputTokens": 10, "outputTokens": 5,
                        "inputTokenDetails": {"cacheReadTokens": 0}}},
    ]
    MockUpstreamHandler.response_status = 200
    MockUpstreamHandler.request_log = []

    server = ThreadedHTTPServer(("127.0.0.1", 0), MockUpstreamHandler)
    server.request_queue_size = 128
    host, port = server.server_address
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()

    yield host, port, MockUpstreamHandler

    server.shutdown()


@pytest.fixture
def proxy_server(patch_auth_and_config, mock_upstream):
    """Start a proxy server pointing at the mock upstream. Returns (host, port)."""
    from command_code_proxy.proxy import ProxyHandler
    upstream_host, upstream_port, _ = mock_upstream
    api_base = f"http://{upstream_host}:{upstream_port}"

    with patch("command_code_proxy.proxy.API_BASE", api_base), \
         patch("command_code_proxy.wire_format.API_BASE", api_base):

        server = ThreadedHTTPServer(("127.0.0.1", 0), ProxyHandler)
        server.daemon_threads = True
        host, port = server.server_address
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()

        yield host, port

        server.shutdown()
