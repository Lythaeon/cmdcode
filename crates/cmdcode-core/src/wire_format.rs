pub use crate::types::{Effort, FinishReason, ModelId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// === OpenAI request types =================================================

#[derive(Debug, Clone, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: Option<String>,
    pub messages: Vec<OpenAiMessage>,
    #[serde(default)]
    pub tools: Option<Vec<OpenAiTool>>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub stream: Option<bool>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub stream_options: Option<StreamOptions>,
    #[serde(default)]
    pub frequency_penalty: Option<f64>,
    #[serde(default)]
    pub presence_penalty: Option<f64>,
    #[serde(default)]
    pub top_p: Option<f64>,
    #[serde(default)]
    pub stop: Option<Vec<String>>,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub n: Option<u32>,
    #[serde(default)]
    pub seed: Option<u64>,
    #[serde(default)]
    pub response_format: Option<serde_json::Value>,
    #[serde(default)]
    pub logprobs: Option<bool>,
    #[serde(default)]
    pub top_logprobs: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StreamOptions {
    #[serde(default)]
    pub include_usage: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAiMessage {
    pub role: String,
    #[serde(default)]
    pub content: Option<serde_json::Value>,
    #[serde(default, alias = "tool_call_id")]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<OpenAiToolCall>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAiTool {
    #[serde(rename = "type")]
    pub tool_type: String,
    #[serde(default)]
    pub function: Option<OpenAiFunction>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, alias = "input_schema")]
    pub input_schema: Option<serde_json::Value>,
    #[serde(default)]
    pub parameters: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAiFunction {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub parameters: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAiToolCall {
    pub id: Option<String>,
    #[serde(default)]
    pub function: Option<OpenAiFunctionRef>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAiFunctionRef {
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

// === Command Code wire types ==============================================

#[derive(Debug, Clone, Serialize)]
pub struct CcRequest {
    pub config: CcConfig,
    pub memory: Option<serde_json::Value>,
    pub taste: Option<serde_json::Value>,
    pub skills: Option<serde_json::Value>,
    pub permission_mode: String,
    pub mode: String,
    pub params: CcParams,
}

#[derive(Debug, Clone, Serialize)]
pub struct CcConfig {
    pub working_dir: String,
    pub date: String,
    pub environment: String,
    pub structure: Vec<String>,
    pub is_git_repo: bool,
    pub current_branch: String,
    pub main_branch: String,
    pub git_status: String,
    pub recent_commits: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CcParams {
    pub model: String,
    pub messages: Vec<CcMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<CcTool>,
    pub max_tokens: u32,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "reasoning_effort")]
    pub reasoning_effort: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role")]
pub enum CcMessage {
    #[serde(rename = "system")]
    System { content: String },
    #[serde(rename = "user")]
    User { content: Vec<CcContentItem> },
    #[serde(rename = "assistant")]
    Assistant { content: Vec<CcContentItem> },
    #[serde(rename = "tool")]
    Tool { content: Vec<CcContentItem> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CcContentItem {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image {
        image: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    #[serde(rename = "tool-call")]
    ToolCall {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool-result")]
    ToolResult {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        output: CcOutput,
    },
    #[serde(rename = "reasoning")]
    Reasoning { text: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CcOutput {
    #[serde(rename = "type")]
    pub output_type: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CcTool {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

// === Translation functions ================================================

/// Convert OpenAI tool definitions to CC wire format.
pub fn wire_tools(tools: &[OpenAiTool]) -> Vec<CcTool> {
    tools
        .iter()
        .map(|t| {
            if t.tool_type == "function" {
                if let Some(ref func) = t.function {
                    CcTool {
                        name: func.name.clone(),
                        description: func.description.clone().unwrap_or_default(),
                        input_schema: func.parameters.clone().unwrap_or_else(
                            || serde_json::json!({"type": "object", "properties": {}}),
                        ),
                    }
                } else {
                    CcTool {
                        name: t.name.clone().unwrap_or_default(),
                        description: t.description.clone().unwrap_or_default(),
                        input_schema: t
                            .input_schema
                            .clone()
                            .or_else(|| t.parameters.clone())
                            .unwrap_or_else(
                                || serde_json::json!({"type": "object", "properties": {}}),
                            ),
                    }
                }
            } else {
                CcTool {
                    name: t.name.clone().unwrap_or_default(),
                    description: t.description.clone().unwrap_or_default(),
                    input_schema: t
                        .input_schema
                        .clone()
                        .or_else(|| t.parameters.clone())
                        .unwrap_or_else(|| serde_json::json!({"type": "object", "properties": {}})),
                }
            }
        })
        .collect()
}

/// Convert OpenAI messages to CC wire format.
pub fn wire_messages(messages: &[OpenAiMessage]) -> Vec<CcMessage> {
    let mut wire = Vec::new();
    let mut tool_name_map: HashMap<String, String> = HashMap::new();

    for msg in messages {
        match msg.role.as_str() {
            "system" => {
                // The upstream only accepts user|assistant|tool in the messages
                // array; the system prompt goes in params.system instead.
            }
            "user" => {
                let items = extract_user_content(&msg.content);
                if !items.is_empty() {
                    wire.push(CcMessage::User { content: items });
                }
            }
            "assistant" => {
                let mut items = Vec::new();

                // Handle content array
                if let Some(ref content) = msg.content {
                    items.extend(extract_assistant_content(content, &mut tool_name_map));
                }

                // Handle tool_calls field
                if let Some(ref tool_calls) = msg.tool_calls {
                    for tc in tool_calls {
                        if let Some(ref func) = tc.function {
                            let name = func.name.clone().unwrap_or_default();
                            let args = func.arguments.clone().unwrap_or_default();
                            let tc_id = tc.id.clone().unwrap_or_default();
                            tool_name_map.insert(tc_id.clone(), name.clone());
                            items.push(CcContentItem::ToolCall {
                                tool_call_id: tc_id,
                                tool_name: name,
                                input: serde_json::from_str::<serde_json::Value>(&args)
                                    .unwrap_or(serde_json::Value::String(args)),
                            });
                        }
                    }
                }

                if !items.is_empty() {
                    wire.push(CcMessage::Assistant { content: items });
                }
            }
            "tool" => {
                let tc_id = msg.tool_call_id.clone().unwrap_or_default();
                let tool_name = tool_name_map
                    .get(&tc_id)
                    .cloned()
                    .unwrap_or_else(|| extract_text_content(&msg.content));

                let items = if let Some(ref content) = msg.content {
                    if let Some(arr) = content.as_array() {
                        arr.iter()
                            .map(|part| {
                                if let Some(obj) = part.as_object() {
                                    if obj.get("type").and_then(|t| t.as_str())
                                        == Some("tool_result")
                                    {
                                        return part.clone();
                                    }
                                }
                                serde_json::json!({
                                    "type": "tool-result",
                                    "toolCallId": tc_id,
                                    "toolName": tool_name,
                                    "output": {
                                        "type": "text",
                                        "value": part.as_str().unwrap_or(&part.to_string()),
                                    }
                                })
                            })
                            .collect()
                    } else {
                        vec![serde_json::json!({
                            "type": "tool-result",
                            "toolCallId": tc_id,
                            "toolName": tool_name,
                            "output": {
                                "type": "text",
                                "value": content.as_str().unwrap_or(""),
                            }
                        })]
                    }
                } else {
                    vec![]
                };

                // Parse tool results, logging any that fail to deserialize
                let mut cc_items = Vec::new();
                for v in items {
                    match serde_json::from_value::<CcContentItem>(v) {
                        Ok(item) => cc_items.push(item),
                        Err(e) => {
                            eprintln!("[cmdcode] warning: failed to parse tool result: {e}");
                        }
                    }
                }
                if !cc_items.is_empty() {
                    wire.push(CcMessage::Tool { content: cc_items });
                }
            }
            _ => {
                let content = extract_text_content(&msg.content);
                wire.push(CcMessage::User {
                    content: vec![CcContentItem::Text { text: content }],
                });
            }
        }
    }

    wire
}

fn extract_text_content(content: &Option<serde_json::Value>) -> String {
    match content {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .filter_map(|p| {
                if let Some(obj) = p.as_object() {
                    if obj.get("type").and_then(|t| t.as_str()) == Some("text") {
                        return obj.get("text").and_then(|t| t.as_str()).map(String::from);
                    }
                }
                p.as_str().map(String::from)
            })
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

fn extract_user_content(content: &Option<serde_json::Value>) -> Vec<CcContentItem> {
    let mut items = Vec::new();
    match content {
        Some(serde_json::Value::String(s)) => {
            items.push(CcContentItem::Text { text: s.clone() });
        }
        Some(serde_json::Value::Array(arr)) => {
            for part in arr {
                if let Some(obj) = part.as_object() {
                    match obj.get("type").and_then(|t| t.as_str()) {
                        Some("text") => {
                            let text = obj
                                .get("text")
                                .and_then(|t| t.as_str())
                                .unwrap_or("")
                                .to_string();
                            items.push(CcContentItem::Text { text });
                        }
                        Some("image") | Some("image_url") => {
                            let url = obj
                                .get("image")
                                .or_else(|| obj.get("image_url"))
                                .and_then(|u| {
                                    if let Some(s) = u.as_str() {
                                        Some(s.to_string())
                                    } else {
                                        u.get("url").and_then(|u| u.as_str()).map(String::from)
                                    }
                                })
                                .unwrap_or_default();
                            let mime = obj
                                .get("mimeType")
                                .and_then(|m| m.as_str())
                                .unwrap_or("image/png")
                                .to_string();
                            items.push(CcContentItem::Image {
                                image: url,
                                mime_type: mime,
                            });
                        }
                        _ => {
                            if let Some(text) = part.as_str() {
                                items.push(CcContentItem::Text {
                                    text: text.to_string(),
                                });
                            }
                        }
                    }
                } else if let Some(text) = part.as_str() {
                    items.push(CcContentItem::Text {
                        text: text.to_string(),
                    });
                }
            }
        }
        _ => {}
    }
    items
}

fn extract_assistant_content(
    content: &serde_json::Value,
    tool_name_map: &mut HashMap<String, String>,
) -> Vec<CcContentItem> {
    let mut items = Vec::new();
    let arr = match content.as_array() {
        Some(a) => a,
        None => {
            if let Some(s) = content.as_str() {
                items.push(CcContentItem::Text {
                    text: s.to_string(),
                });
            }
            return items;
        }
    };

    for part in arr {
        if let Some(obj) = part.as_object() {
            match obj.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    let text = obj
                        .get("text")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();
                    items.push(CcContentItem::Text { text });
                }
                Some("tool_call") | Some("tool-call") => {
                    let name = obj
                        .get("name")
                        .or_else(|| obj.get("function").and_then(|f| f.get("name")))
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    let args = obj
                        .get("arguments")
                        .or_else(|| obj.get("input"))
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    let tc_id = obj.get("id").and_then(|i| i.as_str()).unwrap_or("");
                    tool_name_map.insert(tc_id.to_string(), name.clone());
                    items.push(CcContentItem::ToolCall {
                        tool_call_id: tc_id.to_string(),
                        tool_name: name,
                        input: args,
                    });
                }
                Some("reasoning") | Some("thinking") => {
                    let text = obj
                        .get("text")
                        .or_else(|| obj.get("thinking"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();
                    items.push(CcContentItem::Reasoning { text });
                }
                _ => {}
            }
        } else if let Some(text) = part.as_str() {
            items.push(CcContentItem::Text {
                text: text.to_string(),
            });
        }
    }
    items
}

// === OpenAI response types ================================================

#[derive(Debug, Clone, Serialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<CompletionChoice>,
    pub usage: Usage,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompletionChoice {
    pub index: u32,
    pub message: ResponseMessage,
    pub finish_reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResponseMessage {
    pub role: String,
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ResponseToolCall>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResponseToolCall {
    pub id: String,
    pub r#type: String,
    pub function: ResponseFunction,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResponseFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    #[serde(default)]
    pub prompt_tokens_details: Option<PromptTokenDetails>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptTokenDetails {
    #[serde(default)]
    pub cached_tokens: u32,
}

/// Build a non-streaming completion response.
pub fn build_completion(
    model: &str,
    text: &str,
    reasoning: &str,
    tool_calls: &[(String, String, serde_json::Value)],
    finish_reason: FinishReason,
    usage: &CcUsage,
) -> ChatCompletionResponse {
    let mut msg = ResponseMessage {
        role: "assistant".into(),
        content: if text.is_empty() {
            None
        } else {
            Some(text.to_string())
        },
        reasoning_content: if reasoning.is_empty() {
            None
        } else {
            Some(reasoning.to_string())
        },
        tool_calls: None,
    };

    if !tool_calls.is_empty() {
        msg.tool_calls = Some(
            tool_calls
                .iter()
                .map(|(id, name, args)| ResponseToolCall {
                    id: id.clone(),
                    r#type: "function".into(),
                    function: ResponseFunction {
                        name: name.clone(),
                        arguments: serde_json::to_string(args).unwrap_or_default(),
                    },
                })
                .collect(),
        );
    }

    ChatCompletionResponse {
        id: format!("chatcmpl-{}", uuid::Uuid::new_v4()),
        object: "chat.completion".into(),
        created: chrono_now_secs(),
        model: model.to_string(),
        choices: vec![CompletionChoice {
            index: 0,
            message: msg,
            finish_reason: finish_reason.to_string(),
        }],
        usage: Usage {
            prompt_tokens: usage.input_tokens,
            completion_tokens: usage.output_tokens,
            total_tokens: usage.input_tokens + usage.output_tokens,
            prompt_tokens_details: Some(PromptTokenDetails {
                cached_tokens: usage.cache_read_tokens,
            }),
        },
    }
}

/// Usage from upstream NDJSON.
#[derive(Debug, Clone, Default)]
pub struct CcUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
}

fn chrono_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

// === Upstream event parsing ===============================================

#[derive(Debug, Clone, Deserialize)]
pub struct UpstreamEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default, alias = "toolCallId")]
    pub tool_call_id: Option<String>,
    #[serde(default, alias = "toolName")]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub input: Option<serde_json::Value>,
    #[serde(default, alias = "finishReason")]
    pub finish_reason: Option<String>,
    #[serde(default, alias = "rawFinishReason")]
    pub raw_finish_reason: Option<String>,
    #[serde(default, alias = "totalUsage")]
    pub total_usage: Option<UpstreamUsage>,
    #[serde(default)]
    pub error: Option<UpstreamError>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpstreamUsage {
    #[serde(default, alias = "inputTokens")]
    pub input_tokens: Option<u32>,
    #[serde(default, alias = "outputTokens")]
    pub output_tokens: Option<u32>,
    #[serde(default, alias = "inputTokenDetails")]
    pub input_token_details: Option<UpstreamTokenDetails>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpstreamTokenDetails {
    #[serde(default, alias = "cacheReadTokens")]
    pub cache_read_tokens: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpstreamError {
    pub message: Option<String>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_wire_tools_openai_format() {
        let tools = vec![OpenAiTool {
            tool_type: "function".into(),
            function: Some(OpenAiFunction {
                name: "get_weather".into(),
                description: Some("Get weather".into()),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {"city": {"type": "string"}}
                })),
            }),
            name: None,
            description: None,
            input_schema: None,
            parameters: None,
        }];

        let result = wire_tools(&tools);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "get_weather");
        assert_eq!(result[0].description, "Get weather");
    }

    #[test]
    fn test_wire_messages_system_user() {
        let messages = vec![
            OpenAiMessage {
                role: "system".into(),
                content: Some(serde_json::Value::String("Be helpful".into())),
                tool_call_id: None,
                tool_calls: None,
            },
            OpenAiMessage {
                role: "user".into(),
                content: Some(serde_json::Value::String("Hello".into())),
                tool_call_id: None,
                tool_calls: None,
            },
        ];

        let wire = wire_messages(&messages);
        // System messages are skipped in the array (they go to params.system)
        assert_eq!(wire.len(), 1);
        match &wire[0] {
            CcMessage::User { content } => {
                assert_eq!(content.len(), 1);
                match &content[0] {
                    CcContentItem::Text { text } => assert_eq!(text, "Hello"),
                    _ => panic!("expected text"),
                }
            }
            _ => panic!("expected user message"),
        }
    }

    #[test]
    fn test_finish_reason_mapping() {
        assert_eq!(FinishReason::from_upstream("stop"), FinishReason::Stop);
        assert_eq!(
            FinishReason::from_upstream("tool_use"),
            FinishReason::ToolCalls
        );
        assert_eq!(
            FinishReason::from_upstream("tool-calls"),
            FinishReason::ToolCalls
        );
        assert_eq!(FinishReason::from_upstream("length"), FinishReason::Length);
        assert_eq!(
            FinishReason::from_upstream("max_tokens"),
            FinishReason::Length
        );
        assert_eq!(FinishReason::from_upstream("unknown"), FinishReason::Stop);
    }

    #[test]
    fn test_parse_upstream_event() {
        let json = r#"{"type":"text-delta","text":"hello"}"#;
        let evt: UpstreamEvent = serde_json::from_str(json).unwrap();
        assert_eq!(evt.event_type, "text-delta");
        assert_eq!(evt.text.as_deref(), Some("hello"));
    }

    #[test]
    fn test_parse_finish_event() {
        let json = r#"{
            "type": "finish",
            "finishReason": "stop",
            "totalUsage": {
                "inputTokens": 10,
                "outputTokens": 5,
                "inputTokenDetails": {"cacheReadTokens": 2}
            }
        }"#;
        let evt: UpstreamEvent = serde_json::from_str(json).unwrap();
        assert_eq!(evt.event_type, "finish");
        assert_eq!(evt.finish_reason.as_deref(), Some("stop"));
        let usage = evt.total_usage.unwrap();
        assert_eq!(usage.input_tokens, Some(10));
        assert_eq!(usage.output_tokens, Some(5));
    }

    #[test]
    fn test_build_completion() {
        let usage = CcUsage {
            input_tokens: 10,
            output_tokens: 5,
            cache_read_tokens: 2,
        };
        let resp = build_completion(
            "xiaomi/mimo-v2.5",
            "hello",
            "",
            &[],
            FinishReason::Stop,
            &usage,
        );
        assert_eq!(resp.model, "xiaomi/mimo-v2.5");
        assert_eq!(resp.choices[0].message.content.as_deref(), Some("hello"));
        assert_eq!(resp.usage.prompt_tokens, 10);
        assert_eq!(resp.usage.completion_tokens, 5);
        assert_eq!(resp.usage.total_tokens, 15);
        assert_eq!(
            resp.usage
                .prompt_tokens_details
                .as_ref()
                .unwrap()
                .cached_tokens,
            2
        );
    }

    #[test]
    fn test_wire_messages_parses_tool_call_json_arguments() {
        // OpenAI sends tool-call arguments as a JSON-encoded string. Command
        // Code expects input as a real JSON value, so the proxy must parse it.
        let messages = vec![OpenAiMessage {
            role: "assistant".into(),
            content: None,
            tool_call_id: None,
            tool_calls: Some(vec![OpenAiToolCall {
                id: Some("call_1".into()),
                function: Some(OpenAiFunctionRef {
                    name: Some("edit_file".into()),
                    arguments: Some(r#"{"path":"src/main.rs"}"#.into()),
                }),
            }]),
        }];

        let wire = wire_messages(&messages);
        let found = wire.iter().find_map(|m| match m {
            CcMessage::Assistant { content } => content.iter().find_map(|c| match c {
                CcContentItem::ToolCall { input, .. } => Some(input),
                _ => None,
            }),
            _ => None,
        });
        assert!(found.is_some(), "expected a tool-call content item");
        let input = found.unwrap();
        assert!(
            input.is_object(),
            "JSON object arguments must parse to an object, got {input}"
        );
        assert_eq!(input["path"], "src/main.rs");
    }

    #[test]
    fn test_wire_messages_tool_input_parse_fallback_to_string() {
        // Unparseable arguments must remain a string, not be dropped.
        let messages = vec![OpenAiMessage {
            role: "assistant".into(),
            content: None,
            tool_call_id: None,
            tool_calls: Some(vec![OpenAiToolCall {
                id: Some("call_2".into()),
                function: Some(OpenAiFunctionRef {
                    name: Some("f".into()),
                    arguments: Some("not-json-{".into()),
                }),
            }]),
        }];

        let wire = wire_messages(&messages);
        let found = wire.iter().find_map(|m| match m {
            CcMessage::Assistant { content } => content.iter().find_map(|c| match c {
                CcContentItem::ToolCall { input, .. } => Some(input),
                _ => None,
            }),
            _ => None,
        });
        assert!(found.is_some(), "expected a tool-call content item");
        assert!(
            found.unwrap().is_string(),
            "unparseable args must stay a string"
        );
    }
}
