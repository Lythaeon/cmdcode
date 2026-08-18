"""Unit tests for wire_format.py — message/tool translation."""

import json
import pytest

from command_code_proxy.wire_format import wire_messages, wire_tools


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

    def test_tool_result_message(self):
        msgs = [{"role": "tool", "tool_call_id": "call_1", "content": "42"}]
        result = wire_messages(msgs)
        assert result[0]["role"] == "tool"
        assert result[0]["content"][0]["type"] == "tool-result"
        assert result[0]["content"][0]["toolCallId"] == "call_1"

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

    def test_assistant_with_reasoning(self):
        msgs = [{"role": "assistant", "content": [
            {"type": "reasoning", "text": "thinking..."},
            {"type": "text", "text": "answer"},
        ]}]
        result = wire_messages(msgs)
        items = result[0]["content"]
        assert items[0]["type"] == "reasoning"
        assert items[1]["type"] == "text"

    def test_tool_result_dict_passthrough(self):
        msgs = [{"role": "tool", "content": [
            {"type": "tool_result", "toolCallId": "c1", "output": {"type": "text", "value": "ok"}},
        ]}]
        result = wire_messages(msgs)
        assert result[0]["content"][0]["type"] == "tool_result"
