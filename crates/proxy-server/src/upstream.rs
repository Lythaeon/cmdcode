use proxy_core::auth::AuthManager;
use proxy_core::config::ProxyConfig;
use proxy_core::error::UpstreamError;
use proxy_core::types::{Effort, FinishReason, ModelId};
use proxy_core::wire_format::{
    build_completion, wire_messages, wire_tools, CcUsage, ChatCompletionRequest, UpstreamEvent,
};
use std::sync::Arc;
use tokio::sync::mpsc;

pub enum UpstreamResponse {
    Json(serde_json::Value),
    Sse {
        rx: mpsc::Receiver<Result<String, String>>,
    },
}

pub async fn forward_request(
    config: &Arc<ProxyConfig>,
    auth: &AuthManager,
    model: &ModelId,
    body: &ChatCompletionRequest,
    effort: Option<Effort>,
) -> Result<UpstreamResponse, UpstreamError> {
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| ".".to_string());

    // Build auth headers
    let headers = auth
        .build_headers(&cwd)
        .await
        .map_err(|e| UpstreamError::Io(std::io::Error::other(e.to_string())))?;

    // Build upstream body
    let wire_msgs = wire_messages(&body.messages);
    let wire_tools = wire_tools(body.tools.as_deref().unwrap_or_default());
    let max_tokens = body.max_tokens.unwrap_or(64000);

    let upstream_body = serde_json::json!({
        "config": build_git_config(&cwd),
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
        }
    });

    // Connect to upstream
    let url = format!("{}/alpha/generate", config.upstream_url);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(config.upstream_timeout_secs))
        .build()
        .map_err(|e| UpstreamError::Io(std::io::Error::other(e.to_string())))?;

    let mut req_builder = client.post(&url);
    for (k, v) in &headers {
        req_builder = req_builder.header(k.as_str(), v.as_str());
    }
    req_builder = req_builder.json(&upstream_body);

    let response = req_builder.send().await.map_err(|e| {
        if e.is_connect() {
            UpstreamError::ConnectionRefused {
                host: "upstream".into(),
                port: 443,
            }
        } else if e.is_timeout() {
            UpstreamError::Timeout {
                timeout_secs: config.upstream_timeout_secs,
            }
        } else {
            UpstreamError::Io(std::io::Error::other(e.to_string()))
        }
    })?;

    let status = response.status().as_u16();
    if status != 200 {
        let body_text = response.text().await.unwrap_or_default();
        if body_text.starts_with('{') {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&body_text) {
                if let Some(err) = val.get("error") {
                    return Err(UpstreamError::HttpError {
                        status,
                        body: err.to_string(),
                    });
                }
            }
        }
        return Err(UpstreamError::HttpError {
            status,
            body: body_text,
        });
    }

    // Check if streaming
    let is_stream = body.stream.unwrap_or(false);

    if !is_stream {
        // Collect non-streaming response
        let text = response.text().await.map_err(|e| UpstreamError::Io(std::io::Error::other(e.to_string())))?;
        let mut text_parts = Vec::new();
        let mut reasoning_parts = Vec::new();
        let mut tool_calls = Vec::new();
        let mut usage = CcUsage::default();
        let mut finish_reason = FinishReason::Stop;

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(evt) = serde_json::from_str::<UpstreamEvent>(line) {
                match evt.event_type.as_str() {
                    "text-delta" => {
                        if let Some(t) = evt.text {
                            text_parts.push(t);
                        }
                    }
                    "reasoning-delta" => {
                        if let Some(t) = evt.text {
                            reasoning_parts.push(t);
                        }
                    }
                    "tool-call" => {
                        let tc_id = evt.tool_call_id.unwrap_or_default();
                        let name = evt.tool_name.unwrap_or_default();
                        let args = evt.input.unwrap_or(serde_json::Value::Null);
                        tool_calls.push((tc_id, name, args));
                    }
                    "finish" => {
                        if let Some(u) = evt.total_usage {
                            usage.input_tokens = u.input_tokens.unwrap_or(0);
                            usage.output_tokens = u.output_tokens.unwrap_or(0);
                            if let Some(details) = u.input_token_details {
                                usage.cache_read_tokens = details.cache_read_tokens.unwrap_or(0);
                            }
                        }
                        let raw = evt.raw_finish_reason.as_deref()
                            .or(evt.finish_reason.as_deref())
                            .unwrap_or("stop");
                        finish_reason = FinishReason::from_upstream(raw);
                    }
                    "error" => {
                        let msg = evt.error
                            .and_then(|e| e.message)
                            .unwrap_or_else(|| "stream error".into());
                        return Err(UpstreamError::HttpError {
                            status: 502,
                            body: msg,
                        });
                    }
                    _ => {}
                }
            }
        }

        let completion = build_completion(
            model.as_str(),
            &text_parts.join(""),
            &reasoning_parts.join(""),
            &tool_calls,
            finish_reason,
            &usage,
        );

        Ok(UpstreamResponse::Json(
            serde_json::to_value(completion).unwrap(),
        ))
    } else {
        // Streaming: spawn reader, return channel
        let (tx, rx) = mpsc::channel(256);
        let stream = response.bytes_stream();

        tokio::spawn(async move {
            use futures::StreamExt;
            let mut buffer = String::new();
            let mut stream = std::pin::pin!(stream);

            while let Some(chunk_result) = stream.next().await {
                match chunk_result {
                    Ok(chunk) => {
                        buffer.push_str(&String::from_utf8_lossy(&chunk));
                        while let Some(newline_pos) = buffer.find('\n') {
                            let line = buffer[..newline_pos].trim().to_string();
                            buffer = buffer[newline_pos + 1..].to_string();
                            if !line.is_empty() {
                                let sse_line = format!("data: {}\n\n", line);
                                if tx.send(Ok(sse_line)).await.is_err() {
                                    return;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e.to_string())).await;
                        return;
                    }
                }
            }
            // Flush remaining buffer
            if !buffer.trim().is_empty() {
                let sse_line = format!("data: {}\n\n", buffer.trim());
                let _ = tx.send(Ok(sse_line)).await;
            }
            let _ = tx.send(Ok("data: [DONE]\n\n".to_string())).await;
        });

        Ok(UpstreamResponse::Sse { rx })
    }
}

fn build_git_config(cwd: &str) -> serde_json::Value {
    let is_dir = std::path::Path::new(cwd).is_dir();
    let structure: Vec<String> = if is_dir {
        std::fs::read_dir(cwd)
            .ok()
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .filter(|name| !name.starts_with('.'))
                    .filter(|name| {
                        !matches!(
                            name.as_str(),
                            "node_modules"
                                | "dist"
                                | "build"
                                | ".git"
                                | ".svn"
                                | ".hg"
                                | "coverage"
                                | ".nyc_output"
                                | ".cache"
                                | "tmp"
                                | "temp"
                                | ".next"
                                | ".nuxt"
                                | "out"
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let is_git = std::process::Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(cwd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let (branch, main_branch, status_str, commits) = if is_git {
        let branch = std::process::Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(cwd)
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_default();

        let main = std::process::Command::new("git")
            .args(["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
            .current_dir(cwd)
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().replace("origin/", ""))
            .unwrap_or_else(|| "main".to_string());

        let status = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(cwd)
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| {
                if s.trim().is_empty() {
                    "Working tree clean".to_string()
                } else {
                    s.trim().to_string()
                }
            })
            .unwrap_or_else(|| "Working tree clean".to_string());

        let commits = std::process::Command::new("git")
            .args(["log", "--oneline", "-3"])
            .current_dir(cwd)
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| {
                s.trim()
                    .split('\n')
                    .map(String::from)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        (branch, main, status, commits)
    } else {
        (String::new(), String::new(), String::new(), Vec::new())
    };

    let date = time::OffsetDateTime::now_utc()
        .format(&time::macros::format_description!("[year]-[month]-[day]"))
        .unwrap_or_default();

    serde_json::json!({
        "workingDir": cwd,
        "date": date,
        "environment": std::env::consts::OS,
        "structure": structure,
        "isGitRepo": is_git,
        "currentBranch": branch,
        "mainBranch": main_branch,
        "gitStatus": status_str,
        "recentCommits": commits,
    })
}

fn extract_system(messages: &[proxy_core::wire_format::OpenAiMessage]) -> Option<String> {
    for msg in messages {
        if msg.role == "system" {
            return Some(match &msg.content {
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
            });
        }
    }
    None
}
