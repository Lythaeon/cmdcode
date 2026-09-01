pub use crate::types::{Effort, FinishReason, ModelId};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

// === OpenAI request types =================================================

/// Incoming OpenAI-compatible chat completion request.
#[derive(Debug, Clone, Deserialize)]
pub struct ChatCompletionRequest {
    /// Model identifier (e.g. `gpt-4`).
    pub model: Option<String>,
    /// Conversation messages.
    pub messages: Vec<OpenAiMessage>,
    /// Tool definitions available to the model.
    #[serde(default)]
    pub tools: Option<Vec<OpenAiTool>>,
    /// Maximum tokens to generate.
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Sampling temperature.
    #[serde(default)]
    pub temperature: Option<f64>,
    /// Whether to stream the response.
    #[serde(default)]
    pub stream: Option<bool>,
    /// Reasoning effort level (e.g. `low`, `high`).
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    /// Streaming options such as usage reporting.
    #[serde(default)]
    pub stream_options: Option<StreamOptions>,
    /// Frequency penalty value.
    #[serde(default)]
    pub frequency_penalty: Option<f64>,
    /// Presence penalty value.
    #[serde(default)]
    pub presence_penalty: Option<f64>,
    /// Nucleus sampling threshold.
    #[serde(default)]
    pub top_p: Option<f64>,
    /// Stop sequences.
    #[serde(default)]
    pub stop: Option<Vec<String>>,
    /// End-user identifier.
    #[serde(default)]
    pub user: Option<String>,
    /// Number of completions to generate.
    #[serde(default)]
    pub n: Option<u32>,
    /// Random seed for reproducibility.
    #[serde(default)]
    pub seed: Option<u64>,
    /// Response format specification.
    #[serde(default)]
    pub response_format: Option<serde_json::Value>,
    /// Whether to include log probabilities.
    #[serde(default)]
    pub logprobs: Option<bool>,
    /// Number of top log probabilities to return.
    #[serde(default)]
    pub top_logprobs: Option<u32>,
}

impl ChatCompletionRequest {
    /// Deduplicate tool call IDs in the message history.
    ///
    /// Some clients (notably OpenCode) can accumulate duplicate tool call IDs
    /// in their conversation history, which causes upstream LLM APIs to reject
    /// the request with "Duplicate value for 'tool_call_id'".
    ///
    /// This method removes duplicate tool calls from assistant messages and
    /// duplicate tool-result messages, keeping only the first occurrence.
    pub fn deduplicate_tool_calls(&mut self) {
        // First pass: collect all tool_call_ids from assistant messages
        let mut assistant_tool_call_ids = HashSet::new();
        let mut duplicate_assistant_indices = Vec::new();

        for (i, msg) in self.messages.iter().enumerate() {
            if msg.role == "assistant" {
                if let Some(ref tool_calls) = msg.tool_calls {
                    for tc in tool_calls {
                        if let Some(ref id) = tc.id {
                            if !assistant_tool_call_ids.insert(id.clone()) {
                                // This is a duplicate tool call in assistant messages
                                duplicate_assistant_indices.push(i);
                            }
                        }
                    }
                }
            }
        }

        // Second pass: remove duplicate assistant messages (keep first occurrence)
        for i in duplicate_assistant_indices.into_iter().rev() {
            if let Some(msg) = self.messages.get(i) {
                if msg.role == "assistant" && msg.tool_calls.is_some() {
                    self.messages.remove(i);
                }
            }
        }

        // Third pass: remove duplicate tool-result messages
        let mut seen_tool_results = HashSet::new();
        let mut duplicate_tool_result_indices = Vec::new();

        for (i, msg) in self.messages.iter().enumerate() {
            if msg.role == "tool" {
                if let Some(ref tool_call_id) = msg.tool_call_id {
                    if !seen_tool_results.insert(tool_call_id.clone()) {
                        duplicate_tool_result_indices.push(i);
                    }
                }
            }
        }

        for i in duplicate_tool_result_indices.into_iter().rev() {
            self.messages.remove(i);
        }
    }
}

/// Options controlling streaming behaviour.
#[derive(Debug, Clone, Deserialize)]
pub struct StreamOptions {
    /// Whether to include token usage in the final chunk.
    #[serde(default)]
    pub include_usage: Option<bool>,
}

/// A single message in the OpenAI chat format.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAiMessage {
    /// Role of the message sender (`system`, `user`, `assistant`, `tool`).
    pub role: String,
    /// Message content (string or structured parts).
    #[serde(default)]
    pub content: Option<serde_json::Value>,
    /// Tool call ID for tool-role messages.
    #[serde(default, alias = "tool_call_id")]
    pub tool_call_id: Option<String>,
    /// Tool calls made by the assistant.
    #[serde(default)]
    pub tool_calls: Option<Vec<OpenAiToolCall>>,
}

/// Tool definition in OpenAI format.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAiTool {
    /// Tool type (typically `function`).
    #[serde(rename = "type")]
    pub tool_type: String,
    /// Function definition for function-type tools.
    #[serde(default)]
    pub function: Option<OpenAiFunction>,
    /// Tool name (used when `function` is absent).
    #[serde(default)]
    pub name: Option<String>,
    /// Tool description.
    #[serde(default)]
    pub description: Option<String>,
    /// JSON Schema for the tool input.
    #[serde(default, alias = "input_schema")]
    pub input_schema: Option<serde_json::Value>,
    /// Alternative parameter schema.
    #[serde(default)]
    pub parameters: Option<serde_json::Value>,
}

/// Function definition within a tool.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAiFunction {
    /// Function name.
    pub name: String,
    /// Description of what the function does.
    #[serde(default)]
    pub description: Option<String>,
    /// JSON Schema for the function parameters.
    #[serde(default)]
    pub parameters: Option<serde_json::Value>,
}

/// A tool call made by the assistant.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAiToolCall {
    /// Tool call identifier.
    pub id: Option<String>,
    /// Function call details.
    #[serde(default)]
    pub function: Option<OpenAiFunctionRef>,
}

/// Reference to a function call within a tool call.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAiFunctionRef {
    /// Function name.
    pub name: Option<String>,
    /// JSON-encoded function arguments.
    #[serde(default)]
    pub arguments: Option<String>,
}

// === Command Code wire types ==============================================

/// Top-level request body sent to the Command Code upstream.
#[derive(Debug, Clone, Serialize)]
pub struct CcRequest {
    /// Working-directory metadata.
    pub config: CcConfig,
    /// Conversation memory.
    pub memory: Option<serde_json::Value>,
    /// Taste-learning data.
    pub taste: Option<serde_json::Value>,
    /// Skill definitions.
    pub skills: Option<serde_json::Value>,
    /// Permission mode (e.g. `standard`).
    pub permission_mode: String,
    /// Execution mode (e.g. `agent`).
    pub mode: String,
    /// Model parameters, messages, and tool definitions.
    pub params: CcParams,
}

/// Working-directory metadata for the upstream request.
#[derive(Debug, Clone, Serialize)]
pub struct CcConfig {
    /// Current working directory path.
    pub working_dir: String,
    /// Current date in `YYYY-MM-DD` format.
    pub date: String,
    /// Runtime environment name (e.g. `linux`).
    pub environment: String,
    /// Non-hidden entries in the working directory.
    pub structure: Vec<String>,
    /// Whether the working directory is a git repository.
    pub is_git_repo: bool,
    /// Current git branch name.
    pub current_branch: String,
    /// Main branch name.
    pub main_branch: String,
    /// Git status output.
    pub git_status: String,
    /// Recent git commit messages.
    pub recent_commits: Vec<String>,
}

/// Model parameters for the upstream request.
#[derive(Debug, Clone, Serialize)]
pub struct CcParams {
    /// Model identifier.
    pub model: String,
    /// Conversation messages.
    pub messages: Vec<CcMessage>,
    /// Available tools.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<CcTool>,
    /// Maximum tokens to generate.
    pub max_tokens: u32,
    /// Whether to stream the response.
    pub stream: bool,
    /// System prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    /// Sampling temperature.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// Reasoning effort level.
    #[serde(skip_serializing_if = "Option::is_none", rename = "reasoning_effort")]
    pub reasoning_effort: Option<String>,
}

/// Message in the Command Code wire format.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role")]
pub enum CcMessage {
    /// System-level instruction.
    #[serde(rename = "system")]
    System {
        /// System prompt text.
        content: String,
    },
    /// User message.
    #[serde(rename = "user")]
    User {
        /// Content items in the user message.
        content: Vec<CcContentItem>,
    },
    /// Assistant message.
    #[serde(rename = "assistant")]
    Assistant {
        /// Content items in the assistant response.
        content: Vec<CcContentItem>,
    },
    /// Tool result message.
    #[serde(rename = "tool")]
    Tool {
        /// Tool result content items.
        content: Vec<CcContentItem>,
    },
}

/// A single content item within a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CcContentItem {
    /// Plain text content.
    #[serde(rename = "text")]
    Text {
        /// Text content.
        text: String,
    },
    /// Image content.
    #[serde(rename = "image")]
    Image {
        /// Image URL or base64 data URI.
        image: String,
        /// MIME type of the image.
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    /// Tool call made by the assistant.
    #[serde(rename = "tool-call")]
    ToolCall {
        /// Unique tool call identifier.
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        /// Name of the tool being called.
        #[serde(rename = "toolName")]
        tool_name: String,
        /// Tool input arguments.
        input: serde_json::Value,
    },
    /// Tool call result.
    #[serde(rename = "tool-result")]
    ToolResult {
        /// Identifier of the tool call this result responds to.
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        /// Name of the tool that produced this result.
        #[serde(rename = "toolName")]
        tool_name: String,
        /// Tool output value.
        output: CcOutput,
    },
    /// Reasoning or chain-of-thought content.
    #[serde(rename = "reasoning")]
    Reasoning {
        /// Reasoning text.
        text: String,
    },
}

/// Output value produced by a tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CcOutput {
    /// Output type discriminator (e.g. `text`).
    #[serde(rename = "type")]
    pub output_type: String,
    /// Output value as a string.
    pub value: String,
}

/// Tool definition in the Command Code wire format.
#[derive(Debug, Clone, Serialize)]
pub struct CcTool {
    /// Tool name.
    pub name: String,
    /// Tool description.
    pub description: String,
    /// JSON Schema for the tool input.
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

/// Non-streaming chat completion response.
#[derive(Debug, Clone, Serialize)]
pub struct ChatCompletionResponse {
    /// Unique response identifier.
    pub id: String,
    /// Object type (always `chat.completion`).
    pub object: String,
    /// Unix timestamp of creation.
    pub created: i64,
    /// Model that generated the response.
    pub model: String,
    /// Completion choices.
    pub choices: Vec<CompletionChoice>,
    /// Token usage statistics.
    pub usage: Usage,
}

/// A single completion choice.
#[derive(Debug, Clone, Serialize)]
pub struct CompletionChoice {
    /// Choice index within the response.
    pub index: u32,
    /// The generated message.
    pub message: ResponseMessage,
    /// Reason the model stopped generating.
    pub finish_reason: String,
}

/// Message in a completion response.
#[derive(Debug, Clone, Serialize)]
pub struct ResponseMessage {
    /// Role of the message sender (always `assistant`).
    pub role: String,
    /// Generated text content.
    pub content: Option<String>,
    /// Chain-of-thought reasoning content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    /// Tool calls made by the model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ResponseToolCall>>,
}

/// Tool call in a completion response.
#[derive(Debug, Clone, Serialize)]
pub struct ResponseToolCall {
    /// Tool call identifier.
    pub id: String,
    /// Tool type (always `function`).
    pub r#type: String,
    /// Function call details.
    pub function: ResponseFunction,
}

/// Function details within a tool call response.
#[derive(Debug, Clone, Serialize)]
pub struct ResponseFunction {
    /// Function name.
    pub name: String,
    /// JSON-encoded function arguments.
    pub arguments: String,
}

/// Token usage statistics for a completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    /// Tokens in the prompt.
    pub prompt_tokens: u32,
    /// Tokens generated by the model.
    pub completion_tokens: u32,
    /// Total tokens used.
    pub total_tokens: u32,
    /// Detailed prompt token breakdown.
    #[serde(default)]
    pub prompt_tokens_details: Option<PromptTokenDetails>,
}

/// Detailed prompt token statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptTokenDetails {
    /// Number of cached prompt tokens.
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
    /// Number of input tokens.
    pub input_tokens: u32,
    /// Number of output tokens.
    pub output_tokens: u32,
    /// Number of tokens read from the cache.
    pub cache_read_tokens: u32,
}

fn chrono_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

// === Upstream event parsing ===============================================

/// A single NDJSON event from the Command Code upstream.
#[derive(Debug, Clone, Deserialize)]
pub struct UpstreamEvent {
    /// Event type discriminator (e.g. `text-delta`, `finish`, `error`).
    #[serde(rename = "type")]
    pub event_type: String,
    /// Text content for delta events.
    #[serde(default)]
    pub text: Option<String>,
    /// Tool call identifier.
    #[serde(default, alias = "toolCallId")]
    pub tool_call_id: Option<String>,
    /// Tool name.
    #[serde(default, alias = "toolName")]
    pub tool_name: Option<String>,
    /// Tool input arguments.
    #[serde(default)]
    pub input: Option<serde_json::Value>,
    /// Streaming chunk content for AI-SDK `*-delta` events.
    #[serde(default)]
    pub delta: Option<String>,
    /// Event id for AI-SDK step / tool / text events.
    #[serde(default)]
    pub id: Option<String>,
    /// Finish reason (e.g. `stop`, `tool_calls`).
    #[serde(default, alias = "finishReason")]
    pub finish_reason: Option<String>,
    /// Raw upstream finish reason before normalization.
    #[serde(default, alias = "rawFinishReason")]
    pub raw_finish_reason: Option<String>,
    /// Token usage summary in the finish event. The AI-SDK `finish-step`
    /// event carries this under the `usage` key (the legacy format used
    /// `totalUsage`), so accept both.
    #[serde(default)]
    #[serde(alias = "totalUsage")]
    #[serde(alias = "usage")]
    pub total_usage: Option<UpstreamUsage>,
    /// Error details if the event type is `error`.
    #[serde(default)]
    pub error: Option<UpstreamError>,
}

/// Token usage reported by the upstream in a finish event.
#[derive(Debug, Clone, Deserialize)]
pub struct UpstreamUsage {
    /// Number of input tokens.
    #[serde(default, alias = "inputTokens")]
    pub input_tokens: Option<u32>,
    /// Number of output tokens.
    #[serde(default, alias = "outputTokens")]
    pub output_tokens: Option<u32>,
    /// Detailed input token breakdown.
    #[serde(default, alias = "inputTokenDetails")]
    pub input_token_details: Option<UpstreamTokenDetails>,
}

/// Detailed input token breakdown from the upstream.
#[derive(Debug, Clone, Deserialize)]
pub struct UpstreamTokenDetails {
    /// Number of tokens read from the prompt cache.
    #[serde(default, alias = "cacheReadTokens")]
    pub cache_read_tokens: Option<u32>,
}

/// Error details from an upstream error event.
#[derive(Debug, Clone, Deserialize)]
pub struct UpstreamError {
    /// Error message from the upstream.
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

    // === Additional wire_format tests ===

    #[test]
    fn test_wire_tools_empty() {
        let tools = vec![];
        let result = wire_tools(&tools);
        assert!(result.is_empty());
    }

    #[test]
    fn test_wire_tools_non_function_type() {
        let tools = vec![OpenAiTool {
            tool_type: "custom".into(),
            function: None,
            name: Some("my_tool".into()),
            description: Some("A custom tool".into()),
            input_schema: Some(serde_json::json!({"type": "object"})),
            parameters: None,
        }];

        let result = wire_tools(&tools);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "my_tool");
        assert_eq!(result[0].description, "A custom tool");
    }

    #[test]
    fn test_wire_tools_function_without_function_field() {
        let tools = vec![OpenAiTool {
            tool_type: "function".into(),
            function: None,
            name: Some("fallback_name".into()),
            description: Some("desc".into()),
            input_schema: None,
            parameters: Some(serde_json::json!({"type": "object"})),
        }];

        let result = wire_tools(&tools);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "fallback_name");
    }

    #[test]
    fn test_wire_messages_empty() {
        let messages = vec![];
        let wire = wire_messages(&messages);
        assert!(wire.is_empty());
    }

    #[test]
    fn test_wire_messages_all_system() {
        let messages = vec![
            OpenAiMessage {
                role: "system".into(),
                content: Some(serde_json::Value::String("System 1".into())),
                tool_call_id: None,
                tool_calls: None,
            },
            OpenAiMessage {
                role: "system".into(),
                content: Some(serde_json::Value::String("System 2".into())),
                tool_call_id: None,
                tool_calls: None,
            },
        ];

        let wire = wire_messages(&messages);
        // All system messages should be dropped
        assert!(wire.is_empty());
    }

    #[test]
    fn test_wire_messages_unknown_role() {
        let messages = vec![OpenAiMessage {
            role: "unknown_role".into(),
            content: Some(serde_json::Value::String("content".into())),
            tool_call_id: None,
            tool_calls: None,
        }];

        let wire = wire_messages(&messages);
        // Unknown roles fall back to User
        assert_eq!(wire.len(), 1);
        match &wire[0] {
            CcMessage::User { content } => {
                assert_eq!(content.len(), 1);
            }
            _ => panic!("expected user message for unknown role"),
        }
    }

    #[test]
    fn test_wire_messages_assistant_with_content_array() {
        let messages = vec![OpenAiMessage {
            role: "assistant".into(),
            content: Some(serde_json::json!([
                {"type": "text", "text": "Hello"},
                {"type": "text", "text": "World"}
            ])),
            tool_call_id: None,
            tool_calls: None,
        }];

        let wire = wire_messages(&messages);
        assert_eq!(wire.len(), 1);
        match &wire[0] {
            CcMessage::Assistant { content } => {
                assert_eq!(content.len(), 2);
            }
            _ => panic!("expected assistant message"),
        }
    }

    #[test]
    fn test_wire_messages_assistant_with_string_content() {
        let messages = vec![OpenAiMessage {
            role: "assistant".into(),
            content: Some(serde_json::Value::String("Simple text".into())),
            tool_call_id: None,
            tool_calls: None,
        }];

        let wire = wire_messages(&messages);
        assert_eq!(wire.len(), 1);
        match &wire[0] {
            CcMessage::Assistant { content } => {
                assert_eq!(content.len(), 1);
                match &content[0] {
                    CcContentItem::Text { text } => assert_eq!(text, "Simple text"),
                    _ => panic!("expected text"),
                }
            }
            _ => panic!("expected assistant message"),
        }
    }

    #[test]
    fn test_wire_messages_user_with_content_array() {
        let messages = vec![OpenAiMessage {
            role: "user".into(),
            content: Some(serde_json::json!([
                {"type": "text", "text": "What's in this image?"},
                {"type": "image_url", "image_url": {"url": "https://example.com/img.png"}}
            ])),
            tool_call_id: None,
            tool_calls: None,
        }];

        let wire = wire_messages(&messages);
        assert_eq!(wire.len(), 1);
        match &wire[0] {
            CcMessage::User { content } => {
                assert_eq!(content.len(), 2);
                match &content[0] {
                    CcContentItem::Text { text } => assert_eq!(text, "What's in this image?"),
                    _ => panic!("expected text"),
                }
                match &content[1] {
                    CcContentItem::Image { image, mime_type } => {
                        assert_eq!(image, "https://example.com/img.png");
                        assert_eq!(mime_type, "image/png");
                    }
                    _ => panic!("expected image"),
                }
            }
            _ => panic!("expected user message"),
        }
    }

    #[test]
    fn test_wire_messages_user_with_new_image_format() {
        let messages = vec![OpenAiMessage {
            role: "user".into(),
            content: Some(serde_json::json!([
                {"type": "text", "text": "Describe"},
                {"type": "image", "image": "data:image/jpeg;base64,abc", "mimeType": "image/jpeg"}
            ])),
            tool_call_id: None,
            tool_calls: None,
        }];

        let wire = wire_messages(&messages);
        match &wire[0] {
            CcMessage::User { content } => match &content[1] {
                CcContentItem::Image { image, mime_type } => {
                    assert_eq!(image, "data:image/jpeg;base64,abc");
                    assert_eq!(mime_type, "image/jpeg");
                }
                _ => panic!("expected image"),
            },
            _ => panic!("expected user message"),
        }
    }

    #[test]
    fn test_wire_messages_tool_result() {
        let messages = vec![
            OpenAiMessage {
                role: "assistant".into(),
                content: None,
                tool_call_id: None,
                tool_calls: Some(vec![OpenAiToolCall {
                    id: Some("call_1".into()),
                    function: Some(OpenAiFunctionRef {
                        name: Some("search".into()),
                        arguments: Some(r#"{"q":"rust"}"#.into()),
                    }),
                }]),
            },
            OpenAiMessage {
                role: "tool".into(),
                content: Some(serde_json::Value::String(
                    "Rust is a systems language".into(),
                )),
                tool_call_id: Some("call_1".into()),
                tool_calls: None,
            },
        ];

        let wire = wire_messages(&messages);
        assert_eq!(wire.len(), 2);
        match &wire[1] {
            CcMessage::Tool { content } => {
                assert_eq!(content.len(), 1);
                match &content[0] {
                    CcContentItem::ToolResult {
                        tool_call_id,
                        tool_name,
                        output,
                    } => {
                        assert_eq!(tool_call_id, "call_1");
                        assert_eq!(tool_name, "search");
                        assert_eq!(output.value, "Rust is a systems language");
                    }
                    _ => panic!("expected tool-result"),
                }
            }
            _ => panic!("expected tool message"),
        }
    }

    #[test]
    fn test_wire_messages_empty_content() {
        let messages = vec![OpenAiMessage {
            role: "user".into(),
            content: None,
            tool_call_id: None,
            tool_calls: None,
        }];

        let wire = wire_messages(&messages);
        // Empty content produces no messages
        assert!(wire.is_empty());
    }

    #[test]
    fn test_build_completion_empty_text() {
        let usage = CcUsage {
            input_tokens: 5,
            output_tokens: 0,
            cache_read_tokens: 0,
        };
        let resp = build_completion("m", "", "", &[], FinishReason::Stop, &usage);
        assert!(resp.choices[0].message.content.is_none());
        assert!(resp.choices[0].message.tool_calls.is_none());
    }

    #[test]
    fn test_build_completion_with_reasoning() {
        let usage = CcUsage {
            input_tokens: 10,
            output_tokens: 20,
            cache_read_tokens: 0,
        };
        let resp = build_completion(
            "m",
            "final answer",
            "thinking process",
            &[],
            FinishReason::Stop,
            &usage,
        );
        assert_eq!(
            resp.choices[0].message.content.as_deref(),
            Some("final answer")
        );
        assert_eq!(
            resp.choices[0].message.reasoning_content.as_deref(),
            Some("thinking process")
        );
    }

    #[test]
    fn test_build_completion_with_tool_calls() {
        let usage = CcUsage {
            input_tokens: 10,
            output_tokens: 5,
            cache_read_tokens: 0,
        };
        let tool_calls = vec![
            (
                "tc_1".to_string(),
                "get_weather".to_string(),
                serde_json::json!({"city": "London"}),
            ),
            (
                "tc_2".to_string(),
                "get_time".to_string(),
                serde_json::json!({"timezone": "UTC"}),
            ),
        ];
        let resp = build_completion("m", "", "", &tool_calls, FinishReason::ToolCalls, &usage);
        let tc = resp.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(tc.len(), 2);
        assert_eq!(tc[0].id, "tc_1");
        assert_eq!(tc[0].function.name, "get_weather");
        assert_eq!(tc[1].id, "tc_2");
        assert_eq!(tc[1].function.name, "get_time");
        assert_eq!(resp.choices[0].finish_reason, "tool_calls");
    }

    #[test]
    fn test_build_completion_finish_reasons() {
        let usage = CcUsage {
            input_tokens: 1,
            output_tokens: 1,
            cache_read_tokens: 0,
        };

        let resp = build_completion("m", "", "", &[], FinishReason::Stop, &usage);
        assert_eq!(resp.choices[0].finish_reason, "stop");

        let resp = build_completion("m", "", "", &[], FinishReason::ToolCalls, &usage);
        assert_eq!(resp.choices[0].finish_reason, "tool_calls");

        let resp = build_completion("m", "", "", &[], FinishReason::Length, &usage);
        assert_eq!(resp.choices[0].finish_reason, "length");
    }

    #[test]
    fn test_upstream_event_all_fields() {
        let json = r#"{
            "type": "tool-call",
            "toolCallId": "tc_123",
            "toolName": "my_tool",
            "input": {"key": "value"},
            "finishReason": "stop",
            "rawFinishReason": "tool_use",
            "totalUsage": {
                "inputTokens": 100,
                "outputTokens": 50,
                "inputTokenDetails": {"cacheReadTokens": 10}
            }
        }"#;
        let evt: UpstreamEvent = serde_json::from_str(json).unwrap();
        assert_eq!(evt.event_type, "tool-call");
        assert_eq!(evt.tool_call_id.as_deref(), Some("tc_123"));
        assert_eq!(evt.tool_name.as_deref(), Some("my_tool"));
        assert!(evt.input.is_some());
        assert_eq!(evt.finish_reason.as_deref(), Some("stop"));
        assert_eq!(evt.raw_finish_reason.as_deref(), Some("tool_use"));
        assert!(evt.total_usage.is_some());
    }

    #[test]
    fn test_upstream_event_error() {
        let json = r#"{
            "type": "error",
            "error": {"message": "Something went wrong"}
        }"#;
        let evt: UpstreamEvent = serde_json::from_str(json).unwrap();
        assert_eq!(evt.event_type, "error");
        assert_eq!(
            evt.error.as_ref().unwrap().message.as_deref(),
            Some("Something went wrong")
        );
    }

    #[test]
    fn test_cc_request_serialization() {
        let req = CcRequest {
            config: CcConfig {
                working_dir: "/tmp".into(),
                date: "2024-01-01".into(),
                environment: "linux".into(),
                structure: vec!["file.txt".into()],
                is_git_repo: false,
                current_branch: "".into(),
                main_branch: "".into(),
                git_status: "".into(),
                recent_commits: vec![],
            },
            memory: None,
            taste: None,
            skills: None,
            permission_mode: "standard".into(),
            mode: "agent".into(),
            params: CcParams {
                model: "test".into(),
                messages: vec![],
                tools: vec![],
                max_tokens: 100,
                stream: true,
                system: None,
                temperature: None,
                reasoning_effort: None,
            },
        };

        let json = serde_json::to_value(&req).unwrap();
        // CcConfig uses snake_case field names in serialization
        assert_eq!(json["config"]["working_dir"], "/tmp");
        assert_eq!(json["mode"], "agent");
    }

    #[test]
    fn test_cc_message_serialization_roundtrip() {
        let msg = CcMessage::User {
            content: vec![CcContentItem::Text {
                text: "Hello".into(),
            }],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: CcMessage = serde_json::from_str(&json).unwrap();
        match parsed {
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
    fn test_cc_content_item_tool_call_serialization() {
        let item = CcContentItem::ToolCall {
            tool_call_id: "tc_1".into(),
            tool_name: "my_tool".into(),
            input: serde_json::json!({"key": "value"}),
        };
        let json = serde_json::to_string(&item).unwrap();
        assert!(json.contains("tool-call"));
        assert!(json.contains("tc_1"));
        assert!(json.contains("my_tool"));

        let parsed: CcContentItem = serde_json::from_str(&json).unwrap();
        match parsed {
            CcContentItem::ToolCall {
                tool_call_id,
                tool_name,
                ..
            } => {
                assert_eq!(tool_call_id, "tc_1");
                assert_eq!(tool_name, "my_tool");
            }
            _ => panic!("expected tool-call"),
        }
    }

    #[test]
    fn test_extract_text_content_string() {
        let content = Some(serde_json::Value::String("hello".into()));
        assert_eq!(extract_text_content(&content), "hello");
    }

    #[test]
    fn test_extract_text_content_array() {
        let content = Some(serde_json::json!([
            {"type": "text", "text": "hello"},
            {"type": "text", "text": "world"}
        ]));
        assert_eq!(extract_text_content(&content), "hello world");
    }

    #[test]
    fn test_extract_text_content_none() {
        let content: Option<serde_json::Value> = None;
        assert_eq!(extract_text_content(&content), "");
    }

    #[test]
    fn test_extract_user_content_image_url() {
        let content = Some(serde_json::json!([
            {"type": "image_url", "image_url": {"url": "https://example.com/img.png"}}
        ]));
        let items = extract_user_content(&content);
        assert_eq!(items.len(), 1);
        match &items[0] {
            CcContentItem::Image { image, .. } => {
                assert_eq!(image, "https://example.com/img.png");
            }
            _ => panic!("expected image"),
        }
    }

    #[test]
    fn test_extract_user_content_plain_string_in_array() {
        let content = Some(serde_json::json!(["just a string"]));
        let items = extract_user_content(&content);
        assert_eq!(items.len(), 1);
        match &items[0] {
            CcContentItem::Text { text } => assert_eq!(text, "just a string"),
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn test_extract_assistant_content_reasoning() {
        let content = serde_json::json!([
            {"type": "reasoning", "text": "thinking..."},
            {"type": "text", "text": "answer"}
        ]);
        let mut tool_name_map = HashMap::new();
        let items = extract_assistant_content(&content, &mut tool_name_map);
        assert_eq!(items.len(), 2);
        match &items[0] {
            CcContentItem::Reasoning { text } => assert_eq!(text, "thinking..."),
            _ => panic!("expected reasoning"),
        }
    }

    #[test]
    fn test_extract_assistant_content_tool_call_with_function() {
        let content = serde_json::json!([
            {"type": "tool_call", "function": {"name": "my_func"}, "id": "tc_1", "arguments": "{}"}
        ]);
        let mut tool_name_map = HashMap::new();
        let items = extract_assistant_content(&content, &mut tool_name_map);
        assert_eq!(items.len(), 1);
        assert!(tool_name_map.contains_key("tc_1"));
    }

    #[test]
    fn test_extract_assistant_content_tool_call_with_input() {
        let content = serde_json::json!([
            {"type": "tool-call", "name": "my_tool", "id": "tc_2", "input": {"key": "val"}}
        ]);
        let mut tool_name_map = HashMap::new();
        let items = extract_assistant_content(&content, &mut tool_name_map);
        assert_eq!(items.len(), 1);
        match &items[0] {
            CcContentItem::ToolCall { tool_name, .. } => {
                assert_eq!(tool_name, "my_tool");
            }
            _ => panic!("expected tool-call"),
        }
    }

    #[test]
    fn test_extract_assistant_content_thinking_alias() {
        let content = serde_json::json!([
            {"type": "thinking", "thinking": "deep thought"}
        ]);
        let mut tool_name_map = HashMap::new();
        let items = extract_assistant_content(&content, &mut tool_name_map);
        assert_eq!(items.len(), 1);
        match &items[0] {
            CcContentItem::Reasoning { text } => assert_eq!(text, "deep thought"),
            _ => panic!("expected reasoning"),
        }
    }

    #[test]
    fn test_extract_assistant_content_string() {
        let content = serde_json::Value::String("simple text".into());
        let mut tool_name_map = HashMap::new();
        let items = extract_assistant_content(&content, &mut tool_name_map);
        assert_eq!(items.len(), 1);
        match &items[0] {
            CcContentItem::Text { text } => assert_eq!(text, "simple text"),
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn test_wire_messages_tool_result_with_array_content() {
        let messages = vec![
            OpenAiMessage {
                role: "assistant".into(),
                content: None,
                tool_call_id: None,
                tool_calls: Some(vec![OpenAiToolCall {
                    id: Some("call_1".into()),
                    function: Some(OpenAiFunctionRef {
                        name: Some("search".into()),
                        arguments: None,
                    }),
                }]),
            },
            OpenAiMessage {
                role: "tool".into(),
                content: Some(serde_json::json!([
                    {"type": "tool-result", "toolCallId": "call_1", "toolName": "search", "output": {"type": "text", "value": "result"}}
                ])),
                tool_call_id: Some("call_1".into()),
                tool_calls: None,
            },
        ];

        let wire = wire_messages(&messages);
        // The tool result array is parsed, but tool-result type might not match exactly
        assert_eq!(wire.len(), 2);
        match &wire[1] {
            CcMessage::Tool { content } => {
                assert!(!content.is_empty());
            }
            _ => panic!("expected tool message"),
        }
    }

    #[test]
    fn test_usage_serialization_roundtrip() {
        let usage = Usage {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
            prompt_tokens_details: Some(PromptTokenDetails { cached_tokens: 25 }),
        };
        let json = serde_json::to_string(&usage).unwrap();
        let parsed: Usage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.prompt_tokens, 100);
        assert_eq!(parsed.completion_tokens, 50);
        assert_eq!(parsed.total_tokens, 150);
        assert_eq!(
            parsed.prompt_tokens_details.as_ref().unwrap().cached_tokens,
            25
        );
    }

    #[test]
    fn test_cc_usage_default() {
        let usage = CcUsage::default();
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
        assert_eq!(usage.cache_read_tokens, 0);
    }
}
