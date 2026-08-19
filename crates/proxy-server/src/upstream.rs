use proxy_core::auth::AuthManager;
use proxy_core::config::ProxyConfig;
use proxy_core::error::UpstreamError;
use proxy_core::types::{Effort, FinishReason, ModelId};
use proxy_core::wire_format::{
    build_completion, wire_messages, wire_tools, CcUsage, ChatCompletionRequest, UpstreamEvent,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Semaphore};

pub enum UpstreamResponse {
    Json(serde_json::Value),
    Sse { rx: mpsc::Receiver<Result<String, String>> },
}

/// Shared upstream client — connection pool + concurrency limiter.
pub struct UpstreamClient {
    pub http: reqwest::Client,
    pub config: Arc<ProxyConfig>,
    pub auth: Arc<AuthManager>,
    pub semaphore: Option<Semaphore>,
}

impl UpstreamClient {
    pub fn new(config: Arc<ProxyConfig>, auth: Arc<AuthManager>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.upstream_timeout_secs))
            .pool_max_idle_per_host(32)
            .pool_idle_timeout(Duration::from_secs(90))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("failed to build HTTP client");

        let semaphore = config.max_concurrent
            .checked_add(1)
            .map(Semaphore::new);

        Self { http, config, auth, semaphore }
    }

    pub async fn forward_request(
        &self,
        model: &ModelId,
        body: &ChatCompletionRequest,
        effort: Option<Effort>,
    ) -> Result<UpstreamResponse, UpstreamError> {
        let _permit = if let Some(ref sem) = self.semaphore {
            Some(sem.acquire().await.map_err(|e| {
                UpstreamError::Io(std::io::Error::other(format!("semaphore closed: {e}")))
            })?)
        } else {
            None
        };

        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| ".".to_string());

        let headers = self.auth
            .build_headers(&cwd)
            .await
            .map_err(|e| UpstreamError::Io(std::io::Error::other(e.to_string())))?;

        let wire_msgs = wire_messages(&body.messages);
        let wire_tools = wire_tools(body.tools.as_deref().unwrap_or_default());
        let max_tokens = body.max_tokens.unwrap_or(64000);

        let upstream_body = serde_json::json!({
            "config": {},
            "memory": null,
            "taste": null,
            "skills": null,
            "permissionMode": "standard",
            "mode": "agent",
            "params": {
                "model": model.as_str(),
                "messages": wire_msgs,
                "tools": wire_tools,
                "max_tokens": max_tokens,
                "stream": true,
                "system": extract_system(&body.messages),
                "temperature": body.temperature,
                "reasoning_effort": effort.map(|e| e.as_str()),
                "frequency_penalty": body.frequency_penalty,
                "presence_penalty": body.presence_penalty,
                "top_p": body.top_p,
                "stop": body.stop.as_ref().map(|s| serde_json::to_value(s).unwrap_or_default()),
                "user": body.user.as_deref(),
            }
        });

        let url = format!("{}/alpha/generate", self.config.upstream_url);

        let mut last_err: Option<UpstreamError> = None;
        let max_attempts = 1 + self.config.max_retries;

        for attempt in 0..max_attempts {
            let mut req_builder = self.http.post(&url);
            for (k, v) in &headers {
                req_builder = req_builder.header(k.as_str(), v.as_str());
            }
            req_builder = req_builder.json(&upstream_body);

            match req_builder.send().await {
                Ok(response) => {
                    let status = response.status().as_u16();
                    if status != 200 {
                        let body_text = response.text().await.unwrap_or_default();
                        if body_text.starts_with('{') {
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&body_text) {
                                if let Some(err) = val.get("error") {
                                    let upstream_err = UpstreamError::HttpError { status, body: err.to_string() };
                                    if is_retryable(status) && attempt + 1 < max_attempts {
                                        last_err = Some(upstream_err);
                                        let backoff = Duration::from_millis(100 * 2u64.pow(attempt as u32));
                                        tokio::time::sleep(backoff).await;
                                        continue;
                                    }
                                    return Err(upstream_err);
                                }
                            }
                        }
                        let upstream_err = UpstreamError::HttpError { status, body: body_text };
                        if is_retryable(status) && attempt + 1 < max_attempts {
                            last_err = Some(upstream_err);
                            let backoff = Duration::from_millis(100 * 2u64.pow(attempt as u32));
                            tokio::time::sleep(backoff).await;
                            continue;
                        }
                        return Err(upstream_err);
                    }

                    if !body.stream.unwrap_or(false) {
                        let text = response.text().await.map_err(|e| UpstreamError::Io(std::io::Error::other(e.to_string())))?;
                        let mut text_parts = Vec::new();
                        let mut reasoning_parts = Vec::new();
                        let mut tool_calls = Vec::new();
                        let mut usage = CcUsage::default();
                        let mut finish_reason = FinishReason::Stop;

                        for line in text.lines() {
                            let line = line.trim();
                            if line.is_empty() { continue; }
                            if let Ok(evt) = serde_json::from_str::<UpstreamEvent>(line) {
                                match evt.event_type.as_str() {
                                    "text-delta" => { if let Some(t) = evt.text { text_parts.push(t); } }
                                    "reasoning-delta" => { if let Some(t) = evt.text { reasoning_parts.push(t); } }
                                    "tool-call" => {
                                        tool_calls.push((
                                            evt.tool_call_id.unwrap_or_default(),
                                            evt.tool_name.unwrap_or_default(),
                                            evt.input.unwrap_or(serde_json::Value::Null),
                                        ));
                                    }
                                    "finish" => {
                                        if let Some(u) = evt.total_usage {
                                            usage.input_tokens = u.input_tokens.unwrap_or(0);
                                            usage.output_tokens = u.output_tokens.unwrap_or(0);
                                            if let Some(d) = u.input_token_details {
                                                usage.cache_read_tokens = d.cache_read_tokens.unwrap_or(0);
                                            }
                                        }
                                        let raw = evt.raw_finish_reason.as_deref()
                                            .or(evt.finish_reason.as_deref())
                                            .unwrap_or("stop");
                                        finish_reason = FinishReason::from_upstream(raw);
                                    }
                                    "error" => {
                                        return Err(UpstreamError::HttpError {
                                            status: 502,
                                            body: evt.error.and_then(|e| e.message).unwrap_or_else(|| "stream error".into()),
                                        });
                                    }
                                    _ => {}
                                }
                            }
                        }

                        return Ok(UpstreamResponse::Json(serde_json::to_value(
                            build_completion(model.as_str(), &text_parts.join(""), &reasoning_parts.join(""), &tool_calls, finish_reason, &usage)
                        ).unwrap()));
                    } else {
                        let (tx, rx) = mpsc::channel(256);
                        let stream = response.bytes_stream();
                        let model_str = model.as_str().to_string();

                        tokio::spawn(async move {
                            use futures::StreamExt;
                            let mut buffer = String::new();
                            let mut stream = std::pin::pin!(stream);
                            let created = chrono_now_secs();
                            let mut tool_index: u32 = 0;

                            while let Some(chunk_result) = stream.next().await {
                                match chunk_result {
                                    Ok(chunk) => {
                                        buffer.push_str(&String::from_utf8_lossy(&chunk));
                                        while let Some(newline_pos) = buffer.find('\n') {
                                            let line = buffer[..newline_pos].trim().to_string();
                                            buffer = buffer[newline_pos + 1..].to_string();
                                            if line.is_empty() { continue; }

                                            let evt = match serde_json::from_str::<UpstreamEvent>(&line) {
                                                Ok(e) => e,
                                                Err(_) => continue,
                                            };

                                            let chunk = match evt.event_type.as_str() {
                                                "text-delta" => {
                                                    let text = evt.text.unwrap_or_default();
                                                    serde_json::json!({
                                                        "id": format!("chatcmpl-{}", uuid::Uuid::new_v4()),
                                                        "object": "chat.completion.chunk",
                                                        "created": created,
                                                        "model": model_str,
                                                        "choices": [{
                                                            "index": 0,
                                                            "delta": {"content": text},
                                                            "finish_reason": serde_json::Value::Null,
                                                        }]
                                                    })
                                                }
                                                "reasoning-delta" => {
                                                    let text = evt.text.unwrap_or_default();
                                                    serde_json::json!({
                                                        "id": format!("chatcmpl-{}", uuid::Uuid::new_v4()),
                                                        "object": "chat.completion.chunk",
                                                        "created": created,
                                                        "model": model_str,
                                                        "choices": [{
                                                            "index": 0,
                                                            "delta": {"reasoning_content": text},
                                                            "finish_reason": serde_json::Value::Null,
                                                        }]
                                                    })
                                                }
                                                "tool-call" => {
                                                    let tc_id = evt.tool_call_id.unwrap_or_default();
                                                    let name = evt.tool_name.unwrap_or_default();
                                                    let args = evt.input.unwrap_or(serde_json::Value::Null);
                                                    let args_str = if args.is_string() {
                                                        args.as_str().unwrap().to_string()
                                                    } else {
                                                        serde_json::to_string(&args).unwrap_or_default()
                                                    };
                                                    let idx = tool_index;
                                                    tool_index += 1;
                                                    serde_json::json!({
                                                        "id": format!("chatcmpl-{}", uuid::Uuid::new_v4()),
                                                        "object": "chat.completion.chunk",
                                                        "created": created,
                                                        "model": model_str,
                                                        "choices": [{
                                                            "index": 0,
                                                            "delta": {
                                                                "tool_calls": [{
                                                                    "index": idx,
                                                                    "id": tc_id,
                                                                    "type": "function",
                                                                    "function": {
                                                                        "name": name,
                                                                        "arguments": args_str,
                                                                    }
                                                                }]
                                                            },
                                                            "finish_reason": serde_json::Value::Null,
                                                        }]
                                                    })
                                                }
                                                "finish" => {
                                                    let raw = evt.raw_finish_reason.as_deref()
                                                        .or(evt.finish_reason.as_deref())
                                                        .unwrap_or("stop");
                                                    let fr = match raw {
                                                        "tool_use" | "tool-calls" | "tool_calls" => "tool_calls",
                                                        "length" | "max_tokens" => "length",
                                                        _ => "stop",
                                                    };
                                                    let mut usage_obj = serde_json::json!({});
                                                    if let Some(u) = evt.total_usage {
                                                        if let Some(d) = u.input_token_details {
                                                            usage_obj = serde_json::json!({
                                                                "prompt_tokens": u.input_tokens.unwrap_or(0),
                                                                "completion_tokens": u.output_tokens.unwrap_or(0),
                                                                "total_tokens": u.input_tokens.unwrap_or(0) + u.output_tokens.unwrap_or(0),
                                                                "prompt_tokens_details": {
                                                                    "cached_tokens": d.cache_read_tokens.unwrap_or(0),
                                                                }
                                                            });
                                                        } else {
                                                            usage_obj = serde_json::json!({
                                                                "prompt_tokens": u.input_tokens.unwrap_or(0),
                                                                "completion_tokens": u.output_tokens.unwrap_or(0),
                                                                "total_tokens": u.input_tokens.unwrap_or(0) + u.output_tokens.unwrap_or(0),
                                                            });
                                                        }
                                                    }
                                                    let mut chunk = serde_json::json!({
                                                        "id": format!("chatcmpl-{}", uuid::Uuid::new_v4()),
                                                        "object": "chat.completion.chunk",
                                                        "created": created,
                                                        "model": model_str,
                                                        "choices": [{
                                                            "index": 0,
                                                            "delta": {},
                                                            "finish_reason": fr,
                                                        }]
                                                    });
                                                    chunk["usage"] = usage_obj;
                                                    chunk
                                                }
                                                "error" => {
                                                    let msg = evt.error.and_then(|e| e.message)
                                                        .unwrap_or_else(|| "stream error".into());
                                                    serde_json::json!({
                                                        "error": {"message": msg, "type": "upstream_error"}
                                                    })
                                                }
                                                _ => continue,
                                            };

                                            if tx.send(Ok(format!("data: {}\n\n", serde_json::to_string(&chunk).unwrap_or_default()))).await.is_err() {
                                                return;
                                            }
                                        }
                                    }
                                    Err(e) => { let _ = tx.send(Err(e.to_string())).await; return; }
                                }
                            }
                            let _ = tx.send(Ok("data: [DONE]\n\n".to_string())).await;
                        });

                        return Ok(UpstreamResponse::Sse { rx });
                    }
                }
                Err(e) => {
                    let upstream_err = if e.is_connect() {
                        UpstreamError::ConnectionRefused { host: "upstream".into(), port: 443 }
                    } else if e.is_timeout() {
                        UpstreamError::Timeout { timeout_secs: self.config.upstream_timeout_secs }
                    } else {
                        UpstreamError::Io(std::io::Error::other(e.to_string()))
                    };

                    if attempt + 1 < max_attempts {
                        last_err = Some(upstream_err);
                        let backoff = Duration::from_millis(100 * 2u64.pow(attempt as u32));
                        tokio::time::sleep(backoff).await;
                        continue;
                    }
                    return Err(upstream_err);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| UpstreamError::Io(std::io::Error::other("max retries exceeded"))))
    }
}

fn is_retryable(status: u16) -> bool {
    matches!(status, 502 | 503 | 504)
}

fn chrono_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn extract_system(messages: &[proxy_core::wire_format::OpenAiMessage]) -> Option<String> {
    for msg in messages {
        if msg.role == "system" {
            return Some(match &msg.content {
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(serde_json::Value::Array(arr)) => arr.iter()
                    .filter_map(|p| {
                        if let Some(obj) = p.as_object() {
                            if obj.get("type").and_then(|t| t.as_str()) == Some("text") {
                                return obj.get("text").and_then(|t| t.as_str()).map(String::from);
                            }
                        }
                        p.as_str().map(String::from)
                    }).collect::<Vec<_>>().join(" "),
                _ => String::new(),
            });
        }
    }
    None
}
