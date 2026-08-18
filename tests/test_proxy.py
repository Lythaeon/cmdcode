"""Unit tests for proxy.py — completion assembly, stream translation, error mapping."""

import json
import pytest

from command_code_proxy.proxy import (
    _completion_json,
    _map_finish_reason,
    _sse_line,
    UpstreamError,
)


class TestMapFinishReason:
    def test_stop(self):
        assert _map_finish_reason("stop") == "stop"

    def test_tool_use(self):
        assert _map_finish_reason("tool_use") == "tool_calls"

    def test_tool_calls(self):
        assert _map_finish_reason("tool_calls") == "tool_calls"

    def test_tool_dash_calls(self):
        assert _map_finish_reason("tool-calls") == "tool_calls"

    def test_length(self):
        assert _map_finish_reason("length") == "length"

    def test_max_tokens(self):
        assert _map_finish_reason("max_tokens") == "length"

    def test_none(self):
        assert _map_finish_reason(None) == "stop"

    def test_empty(self):
        assert _map_finish_reason("") == "stop"

    def test_unknown(self):
        assert _map_finish_reason("something_else") == "stop"

    def test_case_insensitive(self):
        assert _map_finish_reason("STOP") == "stop"
        assert _map_finish_reason("Tool_Use") == "tool_calls"


class TestCompletionJson:
    def test_basic_completion(self):
        result = _completion_json("gpt-5.6-luna", "Hello", "", [], "stop",
                                   {"inputTokens": 10, "outputTokens": 5,
                                    "cacheReadTokens": 0})
        assert result["object"] == "chat.completion"
        assert result["model"] == "gpt-5.6-luna"
        assert result["choices"][0]["message"]["content"] == "Hello"
        assert result["choices"][0]["finish_reason"] == "stop"
        assert result["usage"]["prompt_tokens"] == 10
        assert result["usage"]["completion_tokens"] == 5
        assert result["usage"]["total_tokens"] == 15

    def test_empty_content(self):
        result = _completion_json("m", "", "", [], "stop", {})
        assert result["choices"][0]["message"]["content"] is None

    def test_with_reasoning(self):
        result = _completion_json("m", "answer", "thinking", [], "stop", {})
        assert result["choices"][0]["message"]["reasoning_content"] == "thinking"

    def test_no_reasoning_if_empty(self):
        result = _completion_json("m", "answer", "", [], "stop", {})
        assert "reasoning_content" not in result["choices"][0]["message"]

    def test_with_tool_calls(self):
        tool_calls = [{"toolCallId": "c1", "toolName": "foo", "input": {"x": 1}}]
        result = _completion_json("m", "", "", tool_calls, "tool_calls", {})
        tc = result["choices"][0]["message"]["tool_calls"]
        assert len(tc) == 1
        assert tc[0]["id"] == "c1"
        assert tc[0]["function"]["name"] == "foo"
        assert json.loads(tc[0]["function"]["arguments"]) == {"x": 1}

    def test_cached_tokens(self):
        result = _completion_json("m", "", "", [], "stop",
                                   {"inputTokens": 100, "outputTokens": 50,
                                    "cacheReadTokens": 30})
        assert result["usage"]["prompt_tokens_details"]["cached_tokens"] == 30

    def test_id_format(self):
        result = _completion_json("m", "", "", [], "stop", {})
        assert result["id"].startswith("chatcmpl-")
        assert len(result["id"]) == 33  # "chatcmpl-" + 24 hex chars


class TestSseLine:
    def test_basic(self):
        result = _sse_line({"delta": {"content": "hi"}})
        assert result.startswith("data: ")
        assert result.endswith("\n\n")
        parsed = json.loads(result[6:-2])
        assert parsed["delta"]["content"] == "hi"

    def test_done(self):
        result = "data: [DONE]\n\n"
        assert result.startswith("data: [DONE]")


class TestUpstreamError:
    def test_error_attributes(self):
        err = UpstreamError(400, {"message": "bad request", "type": "invalid"})
        assert err.status == 400
        assert err.error["message"] == "bad request"
        assert str(err) == "bad request"

    def test_default_message(self):
        err = UpstreamError(500, {})
        assert str(err) == "upstream error"
