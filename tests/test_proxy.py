"""Unit tests for proxy.py — completion assembly, stream translation, error mapping."""

import json
import pytest

from command_code_proxy.proxy import (
    _completion_json,
    _map_finish_reason,
    _sse_line,
    UpstreamError,
    UpstreamUnavailableError,
    _is_model_allowed,
    _is_retryable,
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

    def test_length_uppercase(self):
        assert _map_finish_reason("LENGTH") == "length"


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

    def test_multiple_tool_calls(self):
        tool_calls = [
            {"toolCallId": "c1", "toolName": "fn1", "input": {}},
            {"toolCallId": "c2", "toolName": "fn2", "input": {"a": 1}},
        ]
        result = _completion_json("m", "", "", tool_calls, "tool_calls", {})
        assert len(result["choices"][0]["message"]["tool_calls"]) == 2

    def test_cached_tokens(self):
        result = _completion_json("m", "", "", [], "stop",
                                   {"inputTokens": 100, "outputTokens": 50,
                                    "cacheReadTokens": 30})
        assert result["usage"]["prompt_tokens_details"]["cached_tokens"] == 30

    def test_id_format(self):
        result = _completion_json("m", "", "", [], "stop", {})
        assert result["id"].startswith("chatcmpl-")
        assert len(result["id"]) == 33  # "chatcmpl-" + 24 hex chars

    def test_created_is_int(self):
        result = _completion_json("m", "", "", [], "stop", {})
        assert isinstance(result["created"], int)

    def test_zero_usage(self):
        result = _completion_json("m", "", "", [], "stop", {})
        assert result["usage"]["prompt_tokens"] == 0
        assert result["usage"]["completion_tokens"] == 0
        assert result["usage"]["total_tokens"] == 0

    def test_tool_calls_string_input(self):
        tool_calls = [{"toolCallId": "c1", "toolName": "fn", "input": "raw-string"}]
        result = _completion_json("m", "", "", tool_calls, "tool_calls", {})
        assert result["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"] == '"raw-string"'


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

    def test_nested_json(self):
        result = _sse_line({"a": {"b": {"c": 1}}})
        parsed = json.loads(result[6:-2])
        assert parsed["a"]["b"]["c"] == 1

    def test_unicode(self):
        result = _sse_line({"text": "\u00e9\u00e8\u00ea"})
        parsed = json.loads(result[6:-2])
        assert parsed["text"] == "\u00e9\u00e8\u00ea"


class TestUpstreamError:
    def test_error_attributes(self):
        err = UpstreamError(400, {"message": "bad request", "type": "invalid"})
        assert err.status == 400
        assert err.error["message"] == "bad request"
        assert str(err) == "bad request"

    def test_default_message(self):
        err = UpstreamError(500, {})
        assert str(err) == "upstream error"

    def test_with_type(self):
        err = UpstreamError(422, {"message": "invalid", "type": "validation_error"})
        assert err.error["type"] == "validation_error"


class TestUpstreamUnavailableError:
    def test_message(self):
        err = UpstreamUnavailableError("connection refused")
        assert str(err) == "connection refused"


class TestIsRetryable:
    def test_502(self):
        assert _is_retryable(502) is True

    def test_503(self):
        assert _is_retryable(503) is True

    def test_504(self):
        assert _is_retryable(504) is True

    def test_500(self):
        assert _is_retryable(500) is False

    def test_400(self):
        assert _is_retryable(400) is False

    def test_401(self):
        assert _is_retryable(401) is False

    def test_200(self):
        assert _is_retryable(200) is False


class TestIsModelAllowed:
    def test_all_allowed_by_default(self):
        assert _is_model_allowed("gpt-5.6-luna") is True
        assert _is_model_allowed("any-model") is True

    def test_allowlist_set(self):
        import command_code_proxy.proxy as pr
        original = pr._allowed_models
        try:
            pr._allowed_models = {"gpt-5.6-luna", "claude-sonnet-5"}
            assert _is_model_allowed("gpt-5.6-luna") is True
            assert _is_model_allowed("claude-sonnet-5") is True
            assert _is_model_allowed("other-model") is False
        finally:
            pr._allowed_models = original

    def test_empty_allowlist_blocks_all(self):
        import command_code_proxy.proxy as pr
        original = pr._allowed_models
        try:
            pr._allowed_models = set()
            assert _is_model_allowed("gpt-5.6-luna") is False
        finally:
            pr._allowed_models = original
