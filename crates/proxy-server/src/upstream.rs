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

use crate::metrics::Metrics;

pub enum UpstreamResponse {
    Json(serde_json::Value),
    Sse {
        rx: mpsc::Receiver<Result<String, String>>,
        cancel: tokio_util::sync::CancellationToken,
    },
}

/// Shared upstream client — connection pool + concurrency limiter.
pub struct UpstreamClient {
    pub http: reqwest::Client,
    pub config: Arc<ProxyConfig>,
    pub auth: Arc<AuthManager>,
    pub metrics: Arc<Metrics>,
    pub semaphore: Option<Arc<Semaphore>>,
}

impl UpstreamClient {
    #[allow(clippy::expect_used)]
    pub fn new(config: Arc<ProxyConfig>, auth: Arc<AuthManager>, metrics: Arc<Metrics>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.upstream_timeout_secs))
            .pool_max_idle_per_host(32)
            .pool_idle_timeout(Duration::from_secs(90))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("failed to build HTTP client");

        let semaphore = if config.max_concurrent == 0 {
            // 0 means unlimited — no concurrency cap.
            None
        } else {
            Some(Arc::new(Semaphore::new(config.max_concurrent)))
        };

        Self {
            http,
            config,
            auth,
            metrics,
            semaphore,
        }
    }

    #[allow(clippy::expect_used)]
    pub async fn forward_request(
        &self,
        model: &ModelId,
        body: &ChatCompletionRequest,
        effort: Option<Effort>,
    ) -> Result<UpstreamResponse, UpstreamError> {
        let _permit = if let Some(sem) = self.semaphore.clone() {
            Some(sem.acquire_owned().await.map_err(|e| {
                UpstreamError::Io(std::io::Error::other(format!("semaphore closed: {e}")))
            })?)
        } else {
            None
        };

        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| ".".to_string());

        let mut headers = self
            .auth
            .build_headers(&cwd)
            .await
            .map_err(|e| UpstreamError::Io(std::io::Error::other(e.to_string())))?;

        let wire_msgs = wire_messages(&body.messages);
        let wire_tools = wire_tools(body.tools.as_deref().unwrap_or_default());
        let max_tokens = body.max_tokens.unwrap_or(64000);

        let mut params = serde_json::json!({
            "model": model.as_str(),
            "messages": wire_msgs,
            "tools": wire_tools,
            "max_tokens": max_tokens,
            "stream": true,
        });
        let params_obj = params.as_object_mut().expect("params is an object");

        if let Some(system) = extract_system(&body.messages) {
            params_obj.insert("system".into(), serde_json::Value::String(system));
        }
        if let Some(t) = body.temperature {
            params_obj.insert("temperature".into(), serde_json::json!(t));
        }
        if let Some(e) = effort {
            params_obj.insert("reasoning_effort".into(), serde_json::json!(e.as_str()));
        }
        if let Some(p) = body.top_p {
            params_obj.insert("top_p".into(), serde_json::json!(p));
        }
        if let Some(fp) = body.frequency_penalty {
            params_obj.insert("frequency_penalty".into(), serde_json::json!(fp));
        }
        if let Some(pp) = body.presence_penalty {
            params_obj.insert("presence_penalty".into(), serde_json::json!(pp));
        }
        if let Some(stop) = &body.stop {
            params_obj.insert(
                "stop".into(),
                serde_json::to_value(stop).unwrap_or_default(),
            );
        }
        if let Some(user) = &body.user {
            params_obj.insert("user".into(), serde_json::json!(user));
        }

        let upstream_body = serde_json::json!({
            "config": build_config(&cwd),
            "memory": null,
            "taste": null,
            "skills": null,
            "permissionMode": "standard",
            "mode": "agent",
            "params": params,
        });

        let url = format!("{}/alpha/generate", self.config.upstream_url);

        let mut last_err: Option<UpstreamError> = None;
        let max_attempts = 1 + self.config.max_retries;
        let mut attempt = 0;
        let mut auth_retried = false;

        while attempt < max_attempts {
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
                                    let upstream_err = UpstreamError::HttpError {
                                        status,
                                        body: err.to_string(),
                                    };
                                    if is_auth_rejected(status) && !auth_retried {
                                        self.auth.invalidate_cache().await;
                                        headers =
                                            self.auth.build_headers(&cwd).await.map_err(|e| {
                                                UpstreamError::Io(std::io::Error::other(
                                                    e.to_string(),
                                                ))
                                            })?;
                                        auth_retried = true;
                                        continue; // refresh once, not against the retry budget
                                    }
                                    if is_retryable(status) && attempt + 1 < max_attempts {
                                        last_err = Some(upstream_err);
                                        let backoff =
                                            Duration::from_millis(100 * 2u64.pow(attempt));
                                        tokio::time::sleep(backoff).await;
                                        self.metrics.inc_retries();
                                        attempt += 1;
                                        continue;
                                    }
                                    return Err(upstream_err);
                                }
                            }
                        }
                        let upstream_err = UpstreamError::HttpError {
                            status,
                            body: body_text,
                        };
                        if is_auth_rejected(status) && !auth_retried {
                            self.auth.invalidate_cache().await;
                            headers = self.auth.build_headers(&cwd).await.map_err(|e| {
                                UpstreamError::Io(std::io::Error::other(e.to_string()))
                            })?;
                            auth_retried = true;
                            continue; // refresh once, not against the retry budget
                        }
                        if is_retryable(status) && attempt + 1 < max_attempts {
                            last_err = Some(upstream_err);
                            let backoff = Duration::from_millis(100 * 2u64.pow(attempt));
                            tokio::time::sleep(backoff).await;
                            self.metrics.inc_retries();
                            attempt += 1;
                            continue;
                        }
                        return Err(upstream_err);
                    }

                    if !body.stream.unwrap_or(false) {
                        let text = response
                            .text()
                            .await
                            .map_err(|e| UpstreamError::Io(std::io::Error::other(e.to_string())))?;
                        let mut text_parts = Vec::new();
                        let mut reasoning_parts = Vec::new();
                        let mut tool_calls = Vec::new();
                        let mut usage = CcUsage::default();
                        let mut finish_reason = FinishReason::Stop;
                        let mut saw_finish = false;

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
                                        tool_calls.push((
                                            evt.tool_call_id.unwrap_or_default(),
                                            evt.tool_name.unwrap_or_default(),
                                            evt.input.unwrap_or(serde_json::Value::Null),
                                        ));
                                    }
                                    "finish" => {
                                        saw_finish = true;
                                        if let Some(u) = evt.total_usage {
                                            usage.input_tokens = u.input_tokens.unwrap_or(0);
                                            usage.output_tokens = u.output_tokens.unwrap_or(0);
                                            if let Some(d) = u.input_token_details {
                                                usage.cache_read_tokens =
                                                    d.cache_read_tokens.unwrap_or(0);
                                            }
                                        }
                                        let raw = evt
                                            .raw_finish_reason
                                            .as_deref()
                                            .or(evt.finish_reason.as_deref())
                                            .unwrap_or("stop");
                                        finish_reason = FinishReason::from_upstream(raw);
                                    }
                                    "error" => {
                                        return Err(UpstreamError::HttpError {
                                            status: 502,
                                            body: evt
                                                .error
                                                .and_then(|e| e.message)
                                                .unwrap_or_else(|| "stream error".into()),
                                        });
                                    }
                                    _ => {}
                                }
                            }
                        }

                        if !saw_finish {
                            return Err(UpstreamError::HttpError {
                                status: 502,
                                body: "upstream ended without finish event".into(),
                            });
                        }

                        return Ok(UpstreamResponse::Json(
                            serde_json::to_value(build_completion(
                                model.as_str(),
                                &text_parts.join(""),
                                &reasoning_parts.join(""),
                                &tool_calls,
                                finish_reason,
                                &usage,
                            ))
                            .map_err(|e| UpstreamError::HttpError {
                                status: 502,
                                body: format!("response serialization: {e}"),
                            })?,
                        ));
                    } else {
                        let (tx, rx) = mpsc::channel(256);
                        let stream = response.bytes_stream();
                        let model_str = model.as_str().to_string();
                        let cancel = tokio_util::sync::CancellationToken::new();
                        let cancel_inner = cancel.clone();
                        let metrics = self.metrics.clone();

                        tokio::spawn(async move {
                            use futures::StreamExt;
                            // Hold the concurrency permit for the full stream
                            // lifetime: it is released when this task exits
                            // (stream ends, errors, cancellation, or the client
                            // gives up), not when forward_request returns.
                            //
                            // NB: must bind to a NAMED variable. `let _ = x;`
                            // would drop the permit immediately; forgetting the
                            // binding entirely would drop it when
                            // forward_request returns. The bound guard lives
                            // until this async block ends, so the semaphore
                            // actually bounds concurrent streams.
                            let _permit_guard = _permit;
                            let _ = metrics;
                            let _ = cancel_inner;
                            let mut buffer: Vec<u8> = Vec::new();
                            // Cursor into `buffer` for the unconsumed portion.
                            // We search from here and only drain the consumed
                            // prefix once per chunk (amortized) instead of
                            // re-copying the whole remaining buffer per line.
                            let mut start = 0usize;
                            let mut stream = std::pin::pin!(stream);
                            let created = chrono_now_secs();
                            let completion_id = format!("chatcmpl-{}", uuid::Uuid::new_v4());
                            let mut state = StreamState {
                                completion_id: &completion_id,
                                created,
                                model: &model_str,
                                tool_index: 0,
                                skipped: 0,
                            };

                            let mut done = false;
                            while !done {
                                tokio::select! {
                                    chunk = stream.next() => {
                                        match chunk {
                                            None => done = true, // clean upstream EOF
                                            Some(Ok(b)) => {
                                                buffer.extend_from_slice(&b);
                                                // Defense against an upstream that
                                                // streams data with no newlines: cap
                                                // the unconsumed buffer so a hostile
                                                // or broken upstream cannot grow it
                                                // without bound.
                                                if buffer.len() - start > MAX_STREAM_BUFFER {
                                                    metrics.inc_truncated_stream();
                                                    return;
                                                }
                                                loop {
                                                    let rel = buffer[start..].iter().position(|&b| b == b'\n');
                                                    match rel {
                                                        Some(rel) => {
                                                            let abs = start + rel;
                                                            // Decode the line bytes to UTF-8 (lossy is OK for individual complete lines)
                                                            let line = std::str::from_utf8(&buffer[start..abs])
                                                                .unwrap_or_default()
                                                                .trim();
                                                            start = abs + 1;
                                                            if line.is_empty() { continue; }
                                                            match translate_line(line, &mut state) {
                                                                LineOutcome::Skip => {}
                                                                LineOutcome::Emit(payload) => {
                                                                    if tx.send(Ok(payload)).await.is_err() {
                                                                        return;
                                                                    }
                                                                }
                                                                LineOutcome::EmitAndStop(payload) => {
                                                                    if tx.send(Ok(payload)).await.is_err() {
                                                                        return;
                                                                    }
                                                                    let _ = tx.send(Ok("data: [DONE]\n\n".to_string())).await;
                                                                    return;
                                                                }
                                                            }
                                                        }
                                                        None => break,
                                                    }
                                                }
                                                if start > 0 {
                                                    buffer.drain(..start);
                                                    start = 0;
                                                }
                                            }
                                            Some(Err(e)) => {
                                                let _ = tx.send(Err(e.to_string())).await;
                                                return;
                                            }
                                        }
                                    },
                                    () = cancel_inner.cancelled() => {
                                        // Downstream client disconnected or the
                                        // idle timer fired: abort immediately.
                                        return;
                                    },
                                }
                            }

                            // Clean EOF: flush any residual unterminated record.
                            let residual = String::from_utf8_lossy(&buffer[start..]).trim().to_string();
                            if residual.is_empty() {
                                let _ = tx.send(Ok("data: [DONE]\n\n".to_string())).await;
                            } else {
                                match translate_line(&residual, &mut state) {
                                    LineOutcome::Skip => {
                                        // Residual did not parse into a complete
                                        // event — this is a truncated stream. Do
                                        // not present it as a clean [DONE].
                                        metrics.inc_truncated_stream();
                                    }
                                    LineOutcome::Emit(payload) => {
                                        if tx.send(Ok(payload)).await.is_ok() {
                                            let _ =
                                                tx.send(Ok("data: [DONE]\n\n".to_string())).await;
                                        }
                                    }
                                    LineOutcome::EmitAndStop(payload) => {
                                        let _ = tx.send(Ok(payload)).await;
                                    }
                                }
                            }

                            if state.skipped > 0 {
                                for _ in 0..state.skipped {
                                    metrics.inc_skipped();
                                }
                            }
                        });

                        return Ok(UpstreamResponse::Sse { rx, cancel });
                    }
                }
                Err(e) => {
                    let upstream_err = if e.is_connect() {
                        UpstreamError::ConnectionRefused {
                            host: "upstream".into(),
                            port: 443,
                        }
                    } else if e.is_timeout() {
                        UpstreamError::Timeout {
                            timeout_secs: self.config.upstream_timeout_secs,
                        }
                    } else {
                        UpstreamError::Io(std::io::Error::other(e.to_string()))
                    };

                    // Retry only on connect failures — timeouts are not retried
                    // (a hung upstream would otherwise multiply the wait by attempts).
                    if e.is_connect() && attempt + 1 < max_attempts {
                        last_err = Some(upstream_err);
                        let backoff = Duration::from_millis(100 * 2u64.pow(attempt));
                        tokio::time::sleep(backoff).await;
                        self.metrics.inc_retries();
                        attempt += 1;
                        continue;
                    }
                    return Err(upstream_err);
                }
            }
        }

        Err(last_err
            .unwrap_or_else(|| UpstreamError::Io(std::io::Error::other("max retries exceeded"))))
    }
}

fn is_retryable(status: u16) -> bool {
    matches!(status, 502..=504)
}

/// A 401/403 upstream response means our cached credential is stale — refresh
/// it and retry once (handled separately from the network-retry budget).
fn is_auth_rejected(status: u16) -> bool {
    status == 401 || status == 403
}

/// Directory listing of `cwd` (non-hidden entries), cached for a short TTL so
/// it is not re-read on every request. `build_config` is called once per
/// upstream request; on a hot loopback this avoids a syscall-heavy `read_dir`
/// each time. The TTL keeps the listing fresh enough to reflect new files.
const STRUCTURE_CACHE_TTL_SECS: u64 = 5;

/// Max unconsumed bytes buffered while waiting for a newline in a stream.
/// Guards against an upstream that never sends `\n` (memory-exhaustion DoS).
const MAX_STREAM_BUFFER: usize = 1024 * 1024;

fn cached_structure(cwd: &str) -> Vec<String> {
    use std::sync::Mutex;

    static CACHE: Mutex<Option<(String, std::time::Instant, Vec<String>)>> = Mutex::new(None);

    let now = std::time::Instant::now();
    let mut guard = match CACHE.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };

    if let Some((cached_cwd, cached_at, cached)) = guard.as_ref() {
        if cached_cwd == cwd && cached_at.elapsed().as_secs() < STRUCTURE_CACHE_TTL_SECS {
            return cached.clone();
        }
    }

    let structure: Vec<String> = std::fs::read_dir(cwd)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| !n.starts_with('.'))
                .collect()
        })
        .unwrap_or_default();

    *guard = Some((cwd.to_string(), now, structure.clone()));
    structure
}

/// Build the config block the upstream requires (workingDir, date, ...).
/// No subprocess calls — git state is reported as clean/non-repo.
fn build_config(cwd: &str) -> serde_json::Value {
    use std::time::SystemTime;

    let date = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| {
            let days = d.as_secs() / 86400;
            let (y, m, day) = civil_from_days(days as i64);
            format!("{y:04}-{m:02}-{day:02}")
        })
        .unwrap_or_default();

    let structure = cached_structure(cwd);

    serde_json::json!({
        "workingDir": cwd,
        "date": date,
        "environment": "linux",
        "structure": structure,
        "isGitRepo": false,
        "currentBranch": "",
        "mainBranch": "",
        "gitStatus": "",
        "recentCommits": [],
    })
}

/// Shared per-stream state for NDJSON -> OpenAI SSE translation.
pub struct StreamState<'a> {
    pub completion_id: &'a str,
    pub created: i64,
    pub model: &'a str,
    pub tool_index: u32,
    /// Number of malformed / unknown upstream events skipped so far.
    pub skipped: u32,
}

/// Result of translating a single upstream NDJSON line.
pub enum LineOutcome {
    /// Line was empty, malformed, or an unknown event type — skip it.
    Skip,
    /// Normal chunk payload (already `data: ...` formatted).
    Emit(String),
    /// Error chunk payload — send it, then `[DONE]` and terminate the stream.
    EmitAndStop(String),
}

/// Translate one upstream NDJSON line into an OpenAI SSE payload.
///
/// Pure function (no I/O) so it can be unit-tested and fuzzed directly.
pub fn translate_line(line: &str, state: &mut StreamState) -> LineOutcome {
    let line = line.trim();
    if line.is_empty() {
        return LineOutcome::Skip;
    }

    let evt = match serde_json::from_str::<UpstreamEvent>(line) {
        Ok(e) => e,
        Err(_) => {
            state.skipped += 1;
            return LineOutcome::Skip;
        }
    };

    let chunk = match evt.event_type.as_str() {
        "text-delta" => {
            let text = evt.text.unwrap_or_default();
            serde_json::json!({
                "id": state.completion_id,
                "object": "chat.completion.chunk",
                "created": state.created,
                "model": state.model,
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
                "id": state.completion_id,
                "object": "chat.completion.chunk",
                "created": state.created,
                "model": state.model,
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
            let args_str = match &args {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => String::new(),
                other => serde_json::to_string(other).unwrap_or_default(),
            };
            let idx = state.tool_index;
            state.tool_index += 1;
            serde_json::json!({
                "id": state.completion_id,
                "object": "chat.completion.chunk",
                "created": state.created,
                "model": state.model,
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
            let raw = evt
                .raw_finish_reason
                .as_deref()
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
                "id": state.completion_id,
                "object": "chat.completion.chunk",
                "created": state.created,
                "model": state.model,
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
            let msg = evt
                .error
                .and_then(|e| e.message)
                .unwrap_or_else(|| "stream error".into());
            let chunk = serde_json::json!({
                "error": {"message": msg, "type": "upstream_error"}
            });
            return LineOutcome::EmitAndStop(format!(
                "data: {}\n\n",
                serde_json::to_string(&chunk).unwrap_or_default()
            ));
        }
        _ => {
            state.skipped += 1;
            return LineOutcome::Skip;
        }
    };

    LineOutcome::Emit(format!(
        "data: {}\n\n",
        serde_json::to_string(&chunk).unwrap_or_default()
    ))
}

/// Days since 1970-01-01 to civil (year, month, day) — Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn state<'a>(completion_id: &'a str, model: &'a str) -> StreamState<'a> {
        StreamState {
            completion_id,
            created: 12345,
            model,
            tool_index: 0,
            skipped: 0,
        }
    }

    #[test]
    fn test_translate_text_delta() {
        let mut s = state("chatcmpl-1", "m1");
        match translate_line(r#"{"type":"text-delta","text":"hello"}"#, &mut s) {
            LineOutcome::Emit(p) => {
                assert!(p.starts_with("data: "));
                assert!(p.contains("\"content\":\"hello\""));
                assert!(p.contains("\"id\":\"chatcmpl-1\""));
                assert!(p.contains("\"model\":\"m1\""));
                assert!(p.contains("\"created\":12345"));
            }
            _ => panic!("expected Emit"),
        }
    }

    #[test]
    fn test_translate_consistent_completion_id_across_chunks() {
        let mut s = state("chatcmpl-fixed", "m1");
        let mut ids = Vec::new();
        for line in [
            r#"{"type":"text-delta","text":"a"}"#,
            r#"{"type":"reasoning-delta","text":"r"}"#,
            r#"{"type":"tool-call","toolCallId":"tc1","toolName":"t","input":{"x":1}}"#,
            r#"{"type":"finish","finishReason":"stop","totalUsage":{"inputTokens":1,"outputTokens":1}}"#,
        ] {
            match translate_line(line, &mut s) {
                LineOutcome::Emit(p) | LineOutcome::EmitAndStop(p) => {
                    let val: serde_json::Value =
                        serde_json::from_str(p.trim_start_matches("data: ").trim()).unwrap();
                    ids.push(val["id"].as_str().unwrap().to_string());
                }
                LineOutcome::Skip => panic!("unexpected skip: {line}"),
            }
        }
        assert_eq!(ids, vec!["chatcmpl-fixed"; 4]);
    }

    #[test]
    fn test_translate_tool_call_string_args_stay_string() {
        let mut s = state("chatcmpl-1", "m1");
        let line = r#"{"type":"tool-call","toolCallId":"tc1","toolName":"get_weather","input":"{\"city\":\"Berlin\"}"}"#;
        match translate_line(line, &mut s) {
            LineOutcome::Emit(p) => {
                let val: serde_json::Value =
                    serde_json::from_str(p.trim_start_matches("data: ").trim()).unwrap();
                let args = &val["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"];
                assert!(
                    args.is_string(),
                    "arguments must be a JSON string on the wire: {args}"
                );
                assert_eq!(args, r#"{"city":"Berlin"}"#);
            }
            _ => panic!("expected Emit"),
        }
    }

    #[test]
    fn test_translate_tool_call_object_args_serialized_to_string() {
        let mut s = state("chatcmpl-1", "m1");
        let line = r#"{"type":"tool-call","toolCallId":"tc1","toolName":"todowrite","input":{"todos":[{"content":"x","priority":"high","status":"in_progress"}]}}"#;
        match translate_line(line, &mut s) {
            LineOutcome::Emit(p) => {
                let val: serde_json::Value =
                    serde_json::from_str(p.trim_start_matches("data: ").trim()).unwrap();
                let args = &val["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"];
                assert!(
                    args.is_string(),
                    "object args must be serialized to a JSON string: {args}"
                );
                let reparsed: serde_json::Value =
                    serde_json::from_str(args.as_str().unwrap()).unwrap();
                assert_eq!(reparsed["todos"][0]["priority"], "high");
            }
            _ => panic!("expected Emit"),
        }
    }

    #[test]
    fn test_translate_tool_call_unparseable_args_stays_string() {
        let mut s = state("chatcmpl-1", "m1");
        let line =
            r#"{"type":"tool-call","toolCallId":"tc1","toolName":"t","input":"not json at all"}"#;
        match translate_line(line, &mut s) {
            LineOutcome::Emit(p) => {
                let val: serde_json::Value =
                    serde_json::from_str(p.trim_start_matches("data: ").trim()).unwrap();
                let args = &val["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"];
                assert_eq!(args, "not json at all");
            }
            _ => panic!("expected Emit"),
        }
    }

    #[test]
    fn test_translate_tool_index_increments() {
        let mut s = state("chatcmpl-1", "m1");
        let mut idxs = Vec::new();
        for line in [
            r#"{"type":"tool-call","toolCallId":"tc1","toolName":"a","input":{}}"#,
            r#"{"type":"tool-call","toolCallId":"tc2","toolName":"b","input":{}}"#,
        ] {
            match translate_line(line, &mut s) {
                LineOutcome::Emit(p) => {
                    let val: serde_json::Value =
                        serde_json::from_str(p.trim_start_matches("data: ").trim()).unwrap();
                    idxs.push(
                        val["choices"][0]["delta"]["tool_calls"][0]["index"]
                            .as_u64()
                            .unwrap(),
                    );
                }
                _ => panic!("expected Emit"),
            }
        }
        assert_eq!(idxs, vec![0, 1]);
    }

    #[test]
    fn test_translate_finish_maps_reasons() {
        for (raw, expected) in [
            ("stop", "stop"),
            ("tool_use", "tool_calls"),
            ("tool-calls", "tool_calls"),
            ("tool_calls", "tool_calls"),
            ("length", "length"),
            ("max_tokens", "length"),
            ("weird", "stop"),
        ] {
            let mut s = state("chatcmpl-1", "m1");
            let line = format!(
                r#"{{"type":"finish","finishReason":"{raw}","totalUsage":{{"inputTokens":2,"outputTokens":3}}}}"#
            );
            match translate_line(&line, &mut s) {
                LineOutcome::Emit(p) => {
                    let val: serde_json::Value =
                        serde_json::from_str(p.trim_start_matches("data: ").trim()).unwrap();
                    assert_eq!(
                        val["choices"][0]["finish_reason"], expected,
                        "for raw={raw}"
                    );
                    assert_eq!(val["usage"]["total_tokens"], 5);
                }
                _ => panic!("expected Emit for {raw}"),
            }
        }
    }

    #[test]
    fn test_translate_finish_usage_details() {
        let mut s = state("chatcmpl-1", "m1");
        let line = r#"{"type":"finish","finishReason":"stop","totalUsage":{"inputTokens":10,"outputTokens":4,"inputTokenDetails":{"cacheReadTokens":7}}}"#;
        match translate_line(line, &mut s) {
            LineOutcome::Emit(p) => {
                let val: serde_json::Value =
                    serde_json::from_str(p.trim_start_matches("data: ").trim()).unwrap();
                assert_eq!(val["usage"]["prompt_tokens"], 10);
                assert_eq!(val["usage"]["completion_tokens"], 4);
                assert_eq!(val["usage"]["total_tokens"], 14);
                assert_eq!(val["usage"]["prompt_tokens_details"]["cached_tokens"], 7);
            }
            _ => panic!("expected Emit"),
        }
    }

    #[test]
    fn test_translate_error_stops_stream() {
        let mut s = state("chatcmpl-1", "m1");
        let line = r#"{"type":"error","error":{"message":"boom"}}"#;
        match translate_line(line, &mut s) {
            LineOutcome::EmitAndStop(p) => {
                assert!(p.contains("\"message\":\"boom\""));
                assert!(p.contains("\"type\":\"upstream_error\""));
            }
            _ => panic!("expected EmitAndStop"),
        }
    }

    #[test]
    fn test_translate_skips_garbage() {
        let mut s = state("chatcmpl-1", "m1");
        for line in [
            "",
            "   ",
            "not json",
            "{\"type\":\"unknown\"}",
            "{broken",
            "null",
            "42",
        ] {
            match translate_line(line, &mut s) {
                LineOutcome::Skip => {}
                _ => panic!("expected Skip for {line:?}"),
            }
        }
    }

    #[test]
    fn test_translate_never_panics_on_malformed_input() {
        let mut s = state("chatcmpl-1", "m1");
        for line in [
            "\u{0}\u{1}\u{2}",
            "{\"type\":\"text-delta\"}",
            "{\"type\":\"tool-call\"}",
            "{\"type\":\"finish\"}",
            "{\"type\":\"error\"}",
            "{\"type\":\"reasoning-delta\",\"text\":null}",
            "{\"type\":\"finish\",\"totalUsage\":{\"inputTokens\":null}}",
        ] {
            let _ = translate_line(line, &mut s);
        }
    }

    /// Randomized soak test over `translate_line`. Ignored by default; run with
    /// `cargo test -p proxy-server -- --ignored fuzz` (or set FUZZ_SECONDS to
    /// bound the run, default 300s). Asserts translate_line never panics and
    /// that every emitted payload is valid SSE carrying a JSON chunk.
    #[test]
    #[ignore = "long-running randomized soak test"]
    fn test_translate_fuzz_soak() {
        use rand::Rng;

        let seconds: u64 = std::env::var("FUZZ_SECONDS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(300);

        let fragments = [
            "",
            " ",
            "\n",
            "\u{0}",
            "{\"type\":",
            "\"text-delta\"",
            "\"tool-call\"",
            "\"finish\"",
            "\"error\"",
            "\"reasoning-delta\"",
            "\"toolCallId\":\"tc\"",
            "\"toolName\":\"t\"",
            "\"input\":",
            "\"text\":\"hi\"",
            "null",
            "42",
            "[1,2,3]",
            "\"totalUsage\":{\"inputTokens\":1,\"outputTokens\":2}",
            "{\"type\":\"finish\",\"finishReason\":\"stop\"}",
            "{\"type\":\"tool-call\",\"input\":{\"a\":[1,{\"b\":null}]}}",
            "{\"type\":\"tool-call\",\"input\":\"{\\\"a\\\":1}\"}",
            "\"reasoning_effort\":\"high\"",
            "}",
        ];

        let mut rng = rand::thread_rng();
        let mut emitted = 0usize;
        let mut parsed_ok = 0usize;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
        let mut iterations = 0u64;

        while std::time::Instant::now() < deadline {
            iterations += 1;
            let mut line = String::new();
            let n_frags = rng.gen_range(1..=8);
            for _ in 0..n_frags {
                line.push_str(fragments[rng.gen_range(0..fragments.len())]);
            }
            let mut s = StreamState {
                completion_id: "chatcmpl-fuzz",
                created: 12345,
                model: "m",
                tool_index: 0,
                skipped: 0,
            };
            match translate_line(&line, &mut s) {
                LineOutcome::Skip => {}
                LineOutcome::Emit(p) | LineOutcome::EmitAndStop(p) => {
                    emitted += 1;
                    assert!(
                        p.starts_with("data: "),
                        "SSE payload must start with data: {p}"
                    );
                    let json = p.trim_start_matches("data: ").trim();
                    assert!(
                        serde_json::from_str::<serde_json::Value>(json).is_ok(),
                        "emitted payload must be valid JSON: {p}"
                    );
                    parsed_ok += 1;
                    let v: serde_json::Value = serde_json::from_str(json).unwrap();
                    if let Some(choices) = v.get("choices") {
                        if let Some(chunk) = choices.as_array().and_then(|a| a.first()) {
                            if let Some(tc) = chunk["delta"]["tool_calls"]
                                .as_array()
                                .and_then(|a| a.first())
                            {
                                let args = &tc["function"]["arguments"];
                                assert!(
                                    args.is_string(),
                                    "tool-call arguments must be a string: {args}"
                                );
                            }
                        }
                    }
                }
            }
        }

        eprintln!(
            "[fuzz] {iterations} iterations, {emitted} emitted, {parsed_ok} parseable in {seconds}s"
        );
        assert!(iterations > 0);
    }

    fn test_config(max_concurrent: usize) -> proxy_core::config::ProxyConfig {
        proxy_core::config::ProxyConfig {
            listen_addr: "127.0.0.1:18080".into(),
            upstream_url: "https://api.commandcode.ai".into(),
            default_model: "xiaomi/mimo-v2.5".into(),
            upstream_timeout_secs: 30,
            max_retries: 0,
            max_concurrent,
            cors_origin: None,
            model_allowlist: None,
            auth_dir: std::path::PathBuf::from("/tmp/test/.commandcode"),
            auth_cache_ttl_secs: 30,
            log_level: "error".into(),
            max_body_size: 10 * 1024 * 1024,
            stream_idle_timeout_secs: 180,
            log_file: None,
            log_max_bytes: 50 * 1024 * 1024,
            log_keep: 5,
            tls_cert: None,
            tls_key: None,
            incoming_token: None,
        }
    }

    #[test]
    fn test_semaphore_zero_means_unlimited() {
        let config = Arc::new(test_config(0));
        let auth = Arc::new(proxy_core::auth::AuthManager::new(
            std::path::PathBuf::from("/tmp/none"),
            30,
        ));
        let metrics = Arc::new(Metrics::new());
        let client = UpstreamClient::new(config, auth, metrics);
        assert!(
            client.semaphore.is_none(),
            "0 concurrent must mean unlimited (None)"
        );
    }

    #[test]
    fn test_semaphore_n_permits_for_n() {
        let config = Arc::new(test_config(5));
        let auth = Arc::new(proxy_core::auth::AuthManager::new(
            std::path::PathBuf::from("/tmp/none"),
            30,
        ));
        let metrics = Arc::new(Metrics::new());
        let client = UpstreamClient::new(config, auth, metrics);
        let sem = client
            .semaphore
            .as_ref()
            .expect("5 must create a semaphore");
        assert_eq!(
            sem.available_permits(),
            5,
            "5 concurrent must allow exactly 5 permits"
        );
    }

    #[tokio::test]
    async fn test_semaphore_permit_released_on_scope_end() {
        let config = Arc::new(test_config(1));
        let auth = Arc::new(proxy_core::auth::AuthManager::new(
            std::path::PathBuf::from("/tmp/none"),
            30,
        ));
        let metrics = Arc::new(Metrics::new());
        let client = UpstreamClient::new(config, auth, metrics);
        let sem = client.semaphore.as_ref().expect("semaphore");

        let permit = sem.clone().acquire_owned().await.unwrap();
        assert_eq!(sem.available_permits(), 0);
        drop(permit);
        assert_eq!(sem.available_permits(), 1, "permit must return on drop");
    }

    #[test]
    fn test_cached_structure_lists_and_excludes_hidden() {
        let tmp =
            std::env::temp_dir().join(format!("cc-proxy-struct-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("file.txt"), "x").unwrap();
        std::fs::write(tmp.join(".hidden"), "y").unwrap();
        std::fs::create_dir(tmp.join("sub")).unwrap();

        let cwd = tmp.display().to_string();
        let listing = cached_structure(&cwd);
        assert!(listing.contains(&"file.txt".to_string()));
        assert!(listing.contains(&"sub".to_string()));
        assert!(
            !listing.contains(&".hidden".to_string()),
            "hidden entries must be excluded"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
