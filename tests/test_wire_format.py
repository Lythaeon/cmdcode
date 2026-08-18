"""Unit tests for wire_format.py — message/tool translation, model catalog."""

import json
import pytest

from command_code_proxy.wire_format import wire_messages, wire_tools, get_model_catalog


class TestModelCatalog:
    def test_catalog_loads_from_cli(self):
        catalog = get_model_catalog()
        assert len(catalog) >= 47  # at least the known set

    def test_all_models_have_required_fields(self):
        for model_id, meta in get_model_catalog().items():
            assert "name" in meta, f"{model_id} missing 'name'"
            assert "reasoning" in meta, f"{model_id} missing 'reasoning'"
            assert "provider" in meta, f"{model_id} missing 'provider'"
            assert isinstance(meta["reasoning"], bool), f"{model_id} reasoning not bool"

    def test_known_models_exist(self):
        expected = [
            "gpt-5.6-luna", "gpt-5.6-sol", "gpt-5.6-terra",
            "claude-sonnet-5", "claude-opus-5",
            "google/gemini-3.7-flash",
            "deepseek/deepseek-v4-pro",
            "xiaomi/mimo-v2.5-pro",
            "xai/grok-4.5",
            "Qwen/Qwen3.8-27B",  # recently added model
        ]
        for m in expected:
            assert m in get_model_catalog(), f"{m} not in catalog"

    def test_providers_are_populated(self):
        providers = {meta["provider"] for meta in get_model_catalog().values()}
        assert len(providers) >= 5
        assert "openai" in providers
        assert "anthropic" in providers
        assert "google" in providers
        assert "open-source" in providers

    def test_efforts_parsed_correctly(self):
        catalog = get_model_catalog()
        # claude-sonnet-5 should have efforts
        assert catalog["claude-sonnet-5"]["efforts"] == ["low", "medium", "high", "xhigh", "max"]
        # gpt-5.6-luna should have efforts
        assert "high" in catalog["gpt-5.6-luna"]["efforts"]
        # haiku has no efforts
        assert catalog["claude-haiku-4-5-20251001"]["efforts"] == []

    def test_context_windows_parsed(self):
        catalog = get_model_catalog()
        assert catalog["gpt-5.6-luna"]["context_window"] == 1_050_000
        assert catalog["claude-sonnet-5"]["context_window"] == 1_000_000
        assert catalog["gpt-5.3-codex"]["context_window"] == 400_000


class TestWireTools:
    def test_empty_tools(self):
        assert wire_tools(None) == []
        assert wire_tools([]) == []

    def test_openai_function_format(self):
        tools = [{
            "type": "function",
            "function": {
                "name": "get_weather",
                "description": "Get weather for a city",
                "parameters": {
                    "type": "object",
                    "properties": {"city": {"type": "string"}},
                    "required": ["city"],
                },
            },
        }]
        result = wire_tools(tools)
        assert len(result) == 1
        assert result[0]["name"] == "get_weather"
        assert result[0]["description"] == "Get weather for a city"
        assert result[0]["input_schema"]["properties"]["city"]["type"] == "string"

    def test_cli_wire_format(self):
        tools = [{
            "name": "get_weather",
            "description": "Get weather",
            "input_schema": {"type": "object", "properties": {}},
        }]
        result = wire_tools(tools)
        assert len(result) == 1
        assert result[0]["name"] == "get_weather"

    def test_openai_format_missing_parameters(self):
        tools = [{
            "type": "function",
            "function": {
                "name": "noop",
                "description": "Does nothing",
            },
        }]
        result = wire_tools(tools)
        assert result[0]["input_schema"] == {"type": "object", "properties": {}}

    def test_multiple_tools(self):
        tools = [
            {"type": "function", "function": {"name": "a", "description": "A"}},
            {"type": "function", "function": {"name": "b", "description": "B"}},
        ]
        result = wire_tools(tools)
        assert len(result) == 2
        assert [t["name"] for t in result] == ["a", "b"]

    def test_mixed_formats(self):
        tools = [
            {"type": "function", "function": {"name": "openai_tool", "description": "OA style"}},
            {"name": "cli_tool", "description": "CLI style", "input_schema": {"type": "object", "properties": {}}},
        ]
        result = wire_tools(tools)
        assert len(result) == 2
        assert result[0]["name"] == "openai_tool"
        assert result[1]["name"] == "cli_tool"

    def test_empty_tool_name(self):
        tools = [{"type": "function", "function": {"name": ""}}]
        result = wire_tools(tools)
        assert result[0]["name"] == ""

    def test_tool_with_complex_schema(self):
        schema = {
            "type": "object",
            "properties": {
                "nested": {
                    "type": "object",
                    "properties": {
                        "deep": {"type": "array", "items": {"type": "string"}},
                    },
                },
            },
        }
        tools = [{"type": "function", "function": {"name": "complex", "parameters": schema}}]
        result = wire_tools(tools)
        assert result[0]["input_schema"]["properties"]["nested"]["properties"]["deep"]["type"] == "array"


class TestWireMessages:
    def test_system_message(self):
        msgs = [{"role": "system", "content": "You are helpful."}]
        result = wire_messages(msgs)
        assert len(result) == 1
        assert result[0] == {"role": "system", "content": "You are helpful."}

    def test_user_string_content(self):
        msgs = [{"role": "user", "content": "Hello"}]
        result = wire_messages(msgs)
        assert len(result) == 1
        assert result[0]["role"] == "user"
        assert result[0]["content"] == [{"type": "text", "text": "Hello"}]

    def test_user_list_content(self):
        msgs = [{"role": "user", "content": [
            {"type": "text", "text": "What is this?"},
        ]}]
        result = wire_messages(msgs)
        assert result[0]["content"] == [{"type": "text", "text": "What is this?"}]

    def test_user_image_content(self):
        msgs = [{"role": "user", "content": [
            {"type": "text", "text": "Describe"},
            {"type": "image_url", "image_url": {"url": "http://example.com/img.png"}},
        ]}]
        result = wire_messages(msgs)
        assert len(result[0]["content"]) == 2
        assert result[0]["content"][1]["type"] == "image"
        assert result[0]["content"][1]["image"] == "http://example.com/img.png"

    def test_user_image_type_field(self):
        msgs = [{"role": "user", "content": [
            {"type": "image", "image": "http://example.com/img.png", "mimeType": "image/jpeg"},
        ]}]
        result = wire_messages(msgs)
        assert result[0]["content"][0]["type"] == "image"
        assert result[0]["content"][0]["mimeType"] == "image/jpeg"

    def test_assistant_string_content(self):
        msgs = [{"role": "assistant", "content": "Sure!"}]
        result = wire_messages(msgs)
        assert result[0]["content"] == [{"type": "text", "text": "Sure!"}]

    def test_assistant_tool_calls(self):
        msgs = [{"role": "assistant", "content": None, "tool_calls": [
            {"id": "call_1", "function": {"name": "foo", "arguments": '{"x":1}'}},
        ]}]
        result = wire_messages(msgs)
        assert len(result[0]["content"]) == 1
        tc = result[0]["content"][0]
        assert tc["type"] == "tool-call"
        assert tc["toolCallId"] == "call_1"
        assert tc["toolName"] == "foo"

    def test_assistant_tool_calls_dict_args(self):
        msgs = [{"role": "assistant", "content": None, "tool_calls": [
            {"id": "call_1", "function": {"name": "foo", "arguments": {"x": 1}}},
        ]}]
        result = wire_messages(msgs)
        tc = result[0]["content"][0]
        assert tc["type"] == "tool-call"
        assert tc["input"] == {"x": 1}

    def test_assistant_tool_calls_invalid_json_args(self):
        msgs = [{"role": "assistant", "content": None, "tool_calls": [
            {"id": "call_1", "function": {"name": "foo", "arguments": "not-json"}},
        ]}]
        result = wire_messages(msgs)
        tc = result[0]["content"][0]
        assert tc["input"] == "not-json"

    def test_assistant_content_parts(self):
        msgs = [{"role": "assistant", "content": [
            {"type": "text", "text": "answer"},
            {"type": "tool_call", "id": "c1", "name": "fn", "input": "{}"},
        ]}]
        result = wire_messages(msgs)
        assert len(result[0]["content"]) == 2
        assert result[0]["content"][0]["type"] == "text"
        assert result[0]["content"][1]["type"] == "tool-call"

    def test_assistant_with_reasoning(self):
        msgs = [{"role": "assistant", "content": [
            {"type": "reasoning", "text": "thinking..."},
            {"type": "text", "text": "answer"},
        ]}]
        result = wire_messages(msgs)
        items = result[0]["content"]
        assert items[0]["type"] == "reasoning"
        assert items[1]["type"] == "text"

    def test_assistant_with_thinking(self):
        msgs = [{"role": "assistant", "content": [
            {"type": "thinking", "thinking": "hmm..."},
            {"type": "text", "text": "answer"},
        ]}]
        result = wire_messages(msgs)
        items = result[0]["content"]
        assert items[0]["type"] == "reasoning"
        assert items[0]["text"] == "hmm..."

    def test_tool_result_message(self):
        msgs = [{"role": "tool", "tool_call_id": "call_1", "content": "42"}]
        result = wire_messages(msgs)
        assert result[0]["role"] == "tool"
        assert result[0]["content"][0]["type"] == "tool-result"
        assert result[0]["content"][0]["toolCallId"] == "call_1"

    def test_tool_result_dict_passthrough(self):
        msgs = [{"role": "tool", "content": [
            {"type": "tool_result", "toolCallId": "c1", "output": {"type": "text", "value": "ok"}},
        ]}]
        result = wire_messages(msgs)
        assert result[0]["content"][0]["type"] == "tool_result"

    def test_tool_result_string_list(self):
        msgs = [{"role": "tool", "tool_call_id": "c1", "content": ["result1", "result2"]}]
        result = wire_messages(msgs)
        assert len(result[0]["content"]) == 2
        assert result[0]["content"][0]["type"] == "tool-result"

    def test_multi_turn_conversation(self):
        msgs = [
            {"role": "system", "content": "Be helpful."},
            {"role": "user", "content": "Hi"},
            {"role": "assistant", "content": "Hello!"},
            {"role": "user", "content": "What's 2+2?"},
            {"role": "assistant", "content": "4"},
            {"role": "user", "content": "Thanks"},
        ]
        result = wire_messages(msgs)
        roles = [m["role"] for m in result]
        assert roles == ["system", "user", "assistant", "user", "assistant", "user"]

    def test_unknown_role_fallback(self):
        msgs = [{"role": "custom", "content": "test"}]
        result = wire_messages(msgs)
        assert result[0]["role"] == "user"
        assert result[0]["content"] == [{"type": "text", "text": "test"}]

    def test_empty_messages(self):
        assert wire_messages([]) == []

    def test_none_content_user(self):
        msgs = [{"role": "user", "content": None}]
        result = wire_messages(msgs)
        # Empty content produces empty items, so no wire message
        assert len(result) == 0

    def test_empty_content_list_user(self):
        msgs = [{"role": "user", "content": []}]
        result = wire_messages(msgs)
        assert len(result) == 0

    def test_string_part_in_user_content(self):
        msgs = [{"role": "user", "content": ["text1", "text2"]}]
        result = wire_messages(msgs)
        assert len(result[0]["content"]) == 2
        assert result[0]["content"][0]["text"] == "text1"

    def test_assistant_string_content_in_list(self):
        msgs = [{"role": "assistant", "content": "hello"}]
        result = wire_messages(msgs)
        assert result[0]["content"] == [{"type": "text", "text": "hello"}]

    def test_assistant_string_part_in_list(self):
        msgs = [{"role": "assistant", "content": ["text1"]}]
        result = wire_messages(msgs)
        assert result[0]["content"][0]["text"] == "text1"
