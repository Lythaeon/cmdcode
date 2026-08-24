use async_trait::async_trait;
use bytes::Bytes;
use pingora_core::upstreams::peer::HttpPeer;
use pingora_error::{Error, ErrorType, Result as PingoraResult};
use pingora_http::RequestHeader;
use pingora_proxy::{ProxyHttp, Session};
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;

use cmdcode_core::auth::AuthManager;
use cmdcode_core::config::ProxyConfig;
use cmdcode_core::model_catalog::get_model_catalog;
use cmdcode_core::rate_limiter::RateLimiter;
use cmdcode_core::types::{Effort, ModelId, RequestId};
use cmdcode_core::wire_format::ChatCompletionRequest;
use tokio_util::sync::CancellationToken;

use crate::metrics::Metrics;
use crate::upstream::{self, UpstreamClient};

/// Which client protocol the request arrived in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Frontend {
    /// `/v1/chat/completions` (identity).
    OpenAi,
    /// `/v1/messages` (Anthropic).
    Anthropic,
    /// `:generateContent` / `:streamGenerateContent` (Gemini).
    Gemini,
    /// `/v1/responses` (OpenAI Responses API).
    Responses,
    /// `/api/chat` (Ollama native).
    Ollama,
}

/// Per-frontend downstream stream renderers. Each consumes the
/// provider-emitted OpenAI SSE payload and produces dialect frames.
enum StreamRenderer {
    Anthropic(cmdcode_core::anthropic_wire::AnthropicStreamRenderer),
    Gemini(cmdcode_core::gemini_wire::GeminiStreamRenderer),
    Responses(cmdcode_core::responses_wire::ResponsesStreamRenderer),
    Ollama(cmdcode_core::ollama_wire::OllamaStreamRenderer),
}

impl StreamRenderer {
    fn feed(&mut self, payload: &str) -> Vec<String> {
        match self {
            Self::Anthropic(r) => r.feed(payload),
            Self::Gemini(r) => r.feed(payload),
            Self::Responses(r) => r.feed(payload),
            Self::Ollama(r) => r.feed(payload),
        }
    }
}

/// Classify an inbound chat path.
fn detect_frontend(path: &str) -> Frontend {
    let p = path.trim_end_matches('/');
    if p.contains(":generateContent") || p.contains(":streamGenerateContent") {
        return Frontend::Gemini;
    }
    match p {
        "/v1/messages" | "/messages" => Frontend::Anthropic,
        "/v1/responses" | "/responses" => Frontend::Responses,
        "/api/chat" => Frontend::Ollama,
        _ => Frontend::OpenAi,
    }
}

/// Pingora proxy handler that forwards requests to Command Code.
pub struct CommandCodeProxy {
    /// Proxy configuration.
    pub config: Arc<ProxyConfig>,
    /// Authentication credential manager.
    pub auth: Arc<AuthManager>,
    /// Shared upstream HTTP client.
    pub upstream_client: Arc<UpstreamClient>,
    /// Request and stream metrics.
    pub metrics: Arc<Metrics>,
    /// Rate limiter for API requests.
    pub rate_limiter: Arc<RateLimiter>,
    /// Server-side response-session store (Responses API chaining).
    pub sessions: Arc<crate::session_store::ResponseSessionStore>,
}

/// Per-request context passed through the proxy pipeline.
pub struct RequestCtx {
    /// Unique identifier for this request.
    pub request_id: RequestId,
    /// Timestamp when the request processing started.
    pub start: Instant,
}

/// Build the pingora HTTP proxy service from the given handler context.
pub fn create_http_proxy_service(
    conf: &Arc<pingora_core::server::configuration::ServerConf>,
    ctx: CommandCodeProxy,
) -> pingora_core::services::listening::Service<pingora_proxy::HttpProxy<CommandCodeProxy, ()>> {
    pingora_proxy::http_proxy_service(conf, ctx)
}

#[async_trait]
impl ProxyHttp for CommandCodeProxy {
    type CTX = RequestCtx;

    fn new_ctx(&self) -> Self::CTX {
        RequestCtx {
            request_id: RequestId::generate(),
            start: Instant::now(),
        }
    }

    async fn upstream_peer(
        &self,
        _session: &mut Session,
        _ctx: &mut Self::CTX,
    ) -> PingoraResult<Box<HttpPeer>> {
        let upstream_url = &self.upstream_client.config.upstream_url;
        let host = upstream_url
            .split("://")
            .nth(1)
            .and_then(|s| s.split(':').next())
            .unwrap_or("api.commandcode.ai")
            .to_string();
        let tls = upstream_url.starts_with("https");

        let peer = HttpPeer::new(
            (
                host.clone(),
                self.upstream_client
                    .config
                    .upstream_url
                    .split(':')
                    .nth(2)
                    .and_then(|s| s.split('/').next())
                    .and_then(|s| s.parse::<u16>().ok())
                    .unwrap_or(if tls { 443 } else { 80 }),
            ),
            tls,
            host,
        );
        Ok(Box::new(peer))
    }

    async fn upstream_request_filter(
        &self,
        _session: &mut Session,
        upstream_request: &mut RequestHeader,
        _ctx: &mut Self::CTX,
    ) -> PingoraResult<()> {
        upstream_request.set_uri(http::Uri::from_static("/alpha/generate"));
        Ok(())
    }

    async fn request_filter(
        &self,
        session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> PingoraResult<bool> {
        let path = session.req_header().uri.path().to_string();
        let method = session.req_header().method.clone();

        // CORS preflight
        if method == http::Method::OPTIONS {
            let mut resp = pingora_http::ResponseHeader::build(204, None)?;
            resp.insert_header("content-type", "text/plain")?;
            if let Some(ref origin) = self.config.cors_origin {
                // F-4: Validate origin is a valid URL scheme (not a regex or wildcard)
                // to prevent CORS bypass via crafted origin strings.
                if origin == "*" || origin.starts_with("http://") || origin.starts_with("https://")
                {
                    resp.insert_header("access-control-allow-origin", origin.as_str())?;
                    resp.insert_header("access-control-allow-methods", "GET, POST, OPTIONS")?;
                    resp.insert_header(
                        "access-control-allow-headers",
                        "Content-Type, Authorization",
                    )?;
                }
            }
            session.write_response_header(Box::new(resp), true).await?;
            return Ok(true);
        }

        match path.trim_end_matches('/') {
            "/v1/models" | "/models" if method == http::Method::GET => {
                self.handle_models(session).await?;
                return Ok(true);
            }
            "/health" if method == http::Method::GET => {
                self.handle_health(session).await?;
                return Ok(true);
            }
            "/metrics" if method == http::Method::GET => {
                self.handle_metrics(session).await?;
                return Ok(true);
            }
            "/v1/chat/completions" | "/chat/completions" if method == http::Method::POST => {
                // OpenAI frontend — continue to upstream
            }
            "/v1/messages" | "/messages" if method == http::Method::POST => {
                // Anthropic frontend — continue to upstream
            }
            "/v1/responses" | "/responses" if method == http::Method::POST => {
                // OpenAI Responses API frontend — continue to upstream
            }
            "/api/chat" if method == http::Method::POST => {
                // Ollama-native chat frontend — continue to upstream
            }
            "/api/tags" if method == http::Method::GET => {
                self.handle_ollama_tags(session).await?;
                return Ok(true);
            }
            p if method == http::Method::POST && p.contains(":generateContent") => {
                // Gemini frontend (non-streaming) — continue to upstream
            }
            p if method == http::Method::POST && p.contains(":streamGenerateContent") => {
                // Gemini frontend (streaming) — continue to upstream
            }
            _ => {
                self.metrics.inc_unknown_route();
                let err = serde_json::json!({
                    "error": {
                        "message": format!("Unknown route {}", path),
                        "type": "not_found"
                    }
                });
                self.send_json(session, 404, &err).await?;
                return Ok(true);
            }
        }

        // Optional incoming-auth gate. /health and /metrics stay open for
        // monitors and scrapers; every other route requires the token.
        let frontend = detect_frontend(&path);
        let api_key = if let Some(ref expected) = self.config.incoming_token {
            let headers = &session.req_header().headers;
            let bearer = headers
                .get(http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .unwrap_or("");
            // Anthropic clients authenticate with x-api-key instead.
            let x_api_key = headers
                .get("x-api-key")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            let (provided, via_key_header) = if !bearer.is_empty() {
                (bearer, false)
            } else if !x_api_key.is_empty() {
                (x_api_key, true)
            } else {
                ("", false)
            };
            let _ = via_key_header;
            if !constant_time_eq(provided.as_bytes(), expected.as_bytes()) {
                self.metrics.inc_error();
                let err = serde_json::json!({
                    "error": {
                        "message": "invalid or missing api key",
                        "type": "invalid_request_error"
                    }
                });
                self.send_json(session, 401, &err).await?;
                return Ok(true);
            }
            provided.to_string()
        } else {
            String::new()
        };

        // Rate limiting check
        if self.config.rate_limit_max_requests > 0
            && !self.rate_limiter.check_rate_limit(&api_key).await
        {
            self.metrics.inc_error();
            let remaining = self.rate_limiter.remaining_requests(&api_key).await;
            let reset = self.rate_limiter.reset_time(&api_key).await;
            tracing::warn!(
                api_key = %mask_api_key(&api_key),
                remaining = remaining,
                reset_secs = reset.as_secs(),
                "rate limit exceeded"
            );
            let err = serde_json::json!({
                "error": {
                    "message": format!("rate limit exceeded, {} requests remaining, resets in {}s", remaining, reset.as_secs()),
                    "type": "rate_limit_error"
                }
            });
            self.send_json(session, 429, &err).await?;
            return Ok(true);
        }

        // Read and parse body with size limit.
        // read_request_body() returns at most one 64KB chunk per call —
        // loop until None to collect the full body.
        let mut body_bytes = Vec::new();
        while let Some(chunk) = session.read_request_body().await? {
            body_bytes.extend_from_slice(&chunk);
            if body_bytes.len() > self.config.max_body_size {
                self.metrics.inc_body_too_large();
                let err = serde_json::json!({
                    "error": {
                        "message": format!("Request body too large: {} bytes (max {})", body_bytes.len(), self.config.max_body_size),
                        "type": "invalid_request_error"
                    }
                });
                self.send_json(session, 413, &err).await?;
                return Ok(true);
            }
        }

        // Frontend adapters convert protocol-specific bodies into the
        // internal OpenAI-format request; the OpenAI frontend parses directly.
        // Frontend adapters convert protocol-specific bodies into the
        // internal OpenAI-format request; the OpenAI frontend parses directly.
        let raw_body: Value = match serde_json::from_slice(&body_bytes) {
            Ok(v) => v,
            Err(e) => {
                self.metrics.inc_bad_requests();
                let err = serde_json::json!({
                    "error": {
                        "message": format!("Invalid JSON body: {}", e),
                        "type": "invalid_request_error"
                    }
                });
                self.send_json(session, 400, &err).await?;
                return Ok(true);
            }
        };

        let mut gemini_model: Option<String> = None;
        let mut anthropic_request: Option<
            cmdcode_core::anthropic_wire::AnthropicRequest,
        > = None;
        let mut responses_request: Option<
            cmdcode_core::responses_wire::ResponsesRequest,
        > = None;
        let mut ollama_request: Option<cmdcode_core::ollama_wire::OllamaChatRequest> =
            None;
        let mut gemini_request: Option<cmdcode_core::gemini_wire::GeminiRequest> = None;

        if frontend == Frontend::Gemini {
            // Path shape: .../models/{model}:generateContent[:stream]
            let model_seg = path
                .rsplit_once("/models/")
                .and_then(|(_, rest)| rest.split(':').next())
                .unwrap_or("");
            gemini_model = Some(model_seg.to_string());
            match serde_json::from_value::<cmdcode_core::gemini_wire::GeminiRequest>(
                raw_body,
            ) {
                Ok(r) => gemini_request = Some(r),
                Err(e) => {
                    self.metrics.inc_bad_requests();
                    self.send_json(
                        session,
                        400,
                        &serde_json::json!({"error": {"code": 400, "message":
                            format!("Invalid request body: {e}"), "status": "INVALID_ARGUMENT"}}),
                    )
                    .await?;
                    return Ok(true);
                }
            }
        } else if frontend == Frontend::Anthropic {
            match serde_json::from_value::<
                cmdcode_core::anthropic_wire::AnthropicRequest,
            >(raw_body)
            {
                Ok(r) => anthropic_request = Some(r),
                Err(e) => {
                    self.metrics.inc_bad_requests();
                    return self
                        .send_json(
                            session,
                            400,
                            &serde_json::json!({
                                "type": "error",
                                "error": {"type": "invalid_request_error",
                                    "message": format!("Invalid request body: {e}")},
                            }),
                        )
                        .await.map(|()| true);
                }
            }
        } else if frontend == Frontend::Responses {
            match serde_json::from_value::<
                cmdcode_core::responses_wire::ResponsesRequest,
            >(raw_body)
            {
                Ok(r) => responses_request = Some(r),
                Err(e) => {
                    self.metrics.inc_bad_requests();
                    return self
                        .send_json(
                            session,
                            400,
                            &serde_json::json!({"error": {"message":
                                format!("Invalid request body: {e}"),
                                "type": "invalid_request_error"}}),
                        )
                        .await.map(|()| true);
                }
            }
        } else if frontend == Frontend::Ollama {
            match serde_json::from_value::<
                cmdcode_core::ollama_wire::OllamaChatRequest,
            >(raw_body)
            {
                Ok(r) => ollama_request = Some(r),
                Err(e) => {
                    self.metrics.inc_bad_requests();
                    return self
                        .send_json(
                            session,
                            400,
                            &serde_json::json!({"error": format!("Invalid request body: {e}")}),
                        )
                        .await.map(|()| true);
                }
            }
        }

        let mut body: ChatCompletionRequest = if let Some(areq) = &anthropic_request {
            areq.to_chat_completion()
        } else if let Some(greq) = &gemini_request {
            let mut cc = greq.to_chat_completion();
            cc.model = gemini_model.clone();
            cc.stream = Some(path.contains(":streamGenerateContent"));
            cc
        } else if let Some(rreq) = &responses_request {
            rreq.to_chat_completion()
        } else if let Some(oreq) = &ollama_request {
            oreq.to_chat_completion()
        } else {
            match serde_json::from_slice(&body_bytes) {
                Ok(b) => b,
                Err(e) => {
                    self.metrics.inc_bad_requests();
                    let err = serde_json::json!({
                        "error": {
                            "message": format!("Invalid JSON body: {}", e),
                            "type": "invalid_request_error"
                        }
                    });
                    self.send_json(session, 400, &err).await?;
                    return Ok(true);
                }
            }
        };

        // Per-frontend semantic validation that serde defaults can mask.
        if frontend == Frontend::Gemini
            && gemini_request
                .as_ref()
                .map(|g| g.contents.is_empty())
                .unwrap_or(true)
        {
            self.metrics.inc_bad_requests();
            self.send_json(
                session,
                400,
                &serde_json::json!({"error": {"code": 400,
                    "message": "contents must contain at least one message",
                    "status": "INVALID_ARGUMENT"}}),
            )
            .await?;
            return Ok(true);
        }
        if frontend == Frontend::Ollama
            && ollama_request
                .as_ref()
                .map(|o| o.messages.is_empty())
                .unwrap_or(true)
        {
            self.metrics.inc_bad_requests();
            self.send_json(
                session,
                400,
                &serde_json::json!({"error": format!(
                    "messages must contain at least one message")}),
            )
            .await?;
            return Ok(true);
        }

        // Responses API server-side state: chain onto a prior response's
        // conversation when previous_response_id is present, and assign our
        // own resp_* id so the client can reference this turn later.
        let mut responses_resp_id: Option<String> = None;
        if let Some(rreq) = &responses_request {
            if let Some(prev_id) = &rreq.previous_response_id {
                let Some(stored) = self.sessions.get(prev_id) else {
                    self.metrics.inc_bad_requests();
                    let err = serde_json::json!({
                        "error": {
                            "message": format!("No response found with id '{prev_id}'."),
                            "type": "invalid_request_error",
                            "param": "previous_response_id",
                        }
                    });
                    self.send_json(session, 404, &err).await?;
                    return Ok(true);
                };
                // Prepend the stored conversation; drop the converted request's
                // own leading system message if the stored one already has it.
                let mut messages = stored;
                let skip_system = messages.first().map(|m| m.role == "system").unwrap_or(false)
                    && body.messages.first().map(|m| m.role == "system").unwrap_or(false);
                for (i, m) in body.messages.into_iter().enumerate() {
                    if i == 0 && skip_system {
                        continue;
                    }
                    messages.push(m);
                }
                body.messages = messages;
            }
            let id = format!("resp_{}", uuid::Uuid::new_v4());
            self.sessions.insert(id.clone(), body.messages.clone());
            responses_resp_id = Some(id);
        }

        // Validate model
        let model_id_str = body.model.as_deref().unwrap_or(&self.config.default_model);
        let (model, effort) = cmdcode_core::types::parse_model_and_effort(model_id_str);
        let model = model.strip_prefix();

        if let Some(ref allowlist) = self.config.model_allowlist {
            if !allowlist.contains(model.as_str()) {
                self.metrics.inc_model_denied();
                let err = serde_json::json!({
                    "error": {
                        "message": format!("Model '{}' is not in the allowed models list", model.as_str()),
                        "type": "invalid_model"
                    }
                });
                self.send_json(session, 400, &err).await?;
                return Ok(true);
            }
        }

        tracing::info!(
            request_id = %ctx.request_id.as_str(),
            model = model.as_str(),
            body_bytes = body_bytes.len(),
            messages = body.messages.len(),
            stream = body.stream.unwrap_or(false),
            "request received"
        );

        let is_stream = body.stream.unwrap_or(false);
        self.metrics.inc_request(is_stream);

        // Forward to upstream
        let start = Instant::now();
        let result = self
            .upstream_client
            .forward_request(&model, &body, effort)
            .await;

        match result {
            Ok(upstream::UpstreamResponse::Json(completion)) => {
                tracing::info!(
                    request_id = %ctx.request_id.as_str(),
                    elapsed_ms = start.elapsed().as_millis() as u64,
                    "completed (non-stream)"
                );
                let out = match frontend {
                    Frontend::Anthropic => cmdcode_core::anthropic_wire::completion_to_anthropic(
                        &completion,
                        model.as_str(),
                    ),
                    Frontend::Responses => {
                        let mut r = cmdcode_core::responses_wire::completion_to_responses(
                            &completion,
                            model.as_str(),
                        );
                        if let Some(id) = &responses_resp_id {
                            r["id"] = serde_json::json!(id);
                        }
                        r
                    }
                    Frontend::Gemini => cmdcode_core::gemini_wire::completion_to_gemini(
                        &completion,
                        model.as_str(),
                    ),
                    Frontend::Ollama => cmdcode_core::ollama_wire::completion_to_ollama(
                        &completion,
                        model.as_str(),
                    ),
                    Frontend::OpenAi => completion,
                };
                self.metrics.inc_bytes_out(out.to_string().len());
                self.send_json(session, 200, &out).await?;
                Ok(true)
            }
            Ok(upstream::UpstreamResponse::Sse { rx, cancel }) => {
                let renderer = match frontend {
                    Frontend::Anthropic => Some(StreamRenderer::Anthropic(
                        cmdcode_core::anthropic_wire::AnthropicStreamRenderer::new(),
                    )),
                    Frontend::Gemini => Some(StreamRenderer::Gemini(
                        cmdcode_core::gemini_wire::GeminiStreamRenderer::new(),
                    )),
                    Frontend::Responses => Some(StreamRenderer::Responses(
                        match &responses_resp_id {
                            Some(id) => cmdcode_core::responses_wire::ResponsesStreamRenderer::new_with_id(id.clone()),
                            None => cmdcode_core::responses_wire::ResponsesStreamRenderer::new(),
                        },
                    )),
                    Frontend::Ollama => Some(StreamRenderer::Ollama(
                        cmdcode_core::ollama_wire::OllamaStreamRenderer::new(model.as_str()),
                    )),
                    Frontend::OpenAi => None,
                };
                self.handle_sse_stream(
                    session,
                    rx,
                    cancel,
                    &model,
                    &body,
                    effort,
                    start,
                    renderer,
                )
                .await
            }
            Err(e) => {
                self.metrics.inc_error();
                if matches!(e, cmdcode_core::error::UpstreamError::Timeout { .. }) {
                    self.metrics.inc_upstream_timeout();
                }
                tracing::error!(error = %e, "upstream error");
                let status = match &e {
                    cmdcode_core::error::UpstreamError::ConnectionRefused { .. } => 502,
                    cmdcode_core::error::UpstreamError::ConnectionReset => 502,
                    cmdcode_core::error::UpstreamError::Timeout { .. } => 504,
                    cmdcode_core::error::UpstreamError::HttpError { status, .. } => *status,
                    _ => 502,
                };
                let err = serde_json::json!({
                    "error": {
                        "message": e.to_string(),
                        "type": "upstream_error"
                    }
                });
                self.send_json(session, status, &err).await?;
                Ok(true)
            }
        }
    }
}

fn is_client_disconnect(e: &pingora_error::Error) -> bool {
    let msg = e.to_string();
    msg.contains("Broken pipe")
        || msg.contains("broken pipe")
        || msg.contains("Connection reset")
        || msg.contains("connection reset")
        || msg.contains("Connection aborted")
        || msg.contains("connection aborted")
        || msg.contains("OperationCanceled")
        || msg.contains("operation was canceled")
}

impl CommandCodeProxy {
    /// Stream an SSE response to the client, retrying empty upstream streams.
    ///
    /// The upstream occasionally accepts a `/alpha/generate` request, emits a
    /// bare `{"type":"start"}` line, then closes the stream with no content and
    /// no `finish` event. The official CLI treats exactly that as a transient
    /// failure and re-invokes the endpoint with backoff (`callModelWithRetry`).
    /// Mirror it here: we hold off writing the 200 SSE header until the first
    /// real chunk arrives, and if a stream closes empty we re-call the upstream
    /// (up to `max_retries` times) before surfacing an error to the client.
    #[allow(clippy::too_many_arguments)]
    async fn handle_sse_stream(
        &self,
        session: &mut Session,
        mut rx: tokio::sync::mpsc::Receiver<Result<String, String>>,
        mut cancel: CancellationToken,
        model: &ModelId,
        body: &ChatCompletionRequest,
        effort: Option<Effort>,
        start: std::time::Instant,
        mut anthropic_renderer: Option<StreamRenderer>,
    ) -> PingoraResult<bool> {
        tracing::info!(
            elapsed_ms = start.elapsed().as_millis() as u64,
            "starting stream"
        );
        self.metrics.stream_started();

        let idle_timeout = std::time::Duration::from_secs(self.config.stream_idle_timeout_secs);
        let max_attempts = 1 + self.config.max_retries;

        // Wait for the first chunk before committing the 200 SSE header. An
        // upstream that closes immediately (only sent {"type":"start"}, or
        // nothing at all) makes rx close with None — that is our retry trigger.
        let mut first_chunk: Option<String> = None;
        let mut abort = false;
        let mut attempts = 0u32;
        while first_chunk.is_none() && !abort && attempts < max_attempts {
            match tokio::time::timeout(idle_timeout, rx.recv()).await {
                Ok(Some(Ok(line))) => {
                    // translate_line above already drops the "start"/session
                    // markers and other no-content events, so the first line we
                    // see here is real content, a finish chunk, or an error.
                    first_chunk = Some(line);
                }
                Ok(Some(Err(e))) => {
                    tracing::error!(error = %e, "upstream stream error before first chunk");
                    self.metrics.inc_error();
                    cancel.cancel();
                    abort = true;
                }
                Ok(None) => {
                    // Empty upstream stream: no content, no finish. The CLI
                    // treats this as transient and retries.
                    self.metrics.inc_empty_stream();
                    cancel.cancel();
                    attempts += 1;
                    if attempts >= max_attempts {
                        tracing::warn!(
                            attempts = attempts,
                            "empty upstream stream after all retries"
                        );
                        let err = serde_json::json!({
                            "error": {
                                "message": "upstream returned an empty stream (no content, no finish event)",
                                "type": "upstream_empty"
                            }
                        });
                        self.send_json(session, 502, &err).await?;
                        self.metrics.stream_finished();
                        return Ok(true);
                    }
                    let backoff = std::time::Duration::from_millis(100 * 2u64.pow(attempts));
                    tracing::warn!(
                        attempt = attempts,
                        backoff_ms = backoff.as_millis(),
                        "empty upstream stream; retrying"
                    );
                    tokio::time::sleep(backoff).await;
                    match self
                        .upstream_client
                        .forward_request(model, body, effort)
                        .await
                    {
                        Ok(upstream::UpstreamResponse::Sse {
                            rx: nrx,
                            cancel: ncan,
                        }) => {
                            rx = nrx;
                            cancel = ncan;
                        }
                        Ok(_) => {
                            tracing::warn!("unexpected non-stream response on retry");
                            cancel.cancel();
                            abort = true;
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "upstream error on stream retry");
                            self.metrics.inc_error();
                            if matches!(e, cmdcode_core::error::UpstreamError::Timeout { .. }) {
                                self.metrics.inc_upstream_timeout();
                            }
                            let status = match &e {
                                cmdcode_core::error::UpstreamError::ConnectionRefused {
                                    ..
                                } => 502,
                                cmdcode_core::error::UpstreamError::ConnectionReset => 502,
                                cmdcode_core::error::UpstreamError::Timeout { .. } => 504,
                                cmdcode_core::error::UpstreamError::HttpError {
                                    status, ..
                                } => *status,
                                _ => 502,
                            };
                            let err = serde_json::json!({
                                "error": {
                                    "message": e.to_string(),
                                    "type": "upstream_error"
                                }
                            });
                            self.send_json(session, status, &err).await?;
                            self.metrics.stream_finished();
                            return Ok(true);
                        }
                    }
                }
                Err(_) => {
                    self.metrics.inc_upstream_timeout();
                    tracing::error!(
                        idle_secs = idle_timeout.as_secs(),
                        "stream idle timeout before first chunk; aborting"
                    );
                    cancel.cancel();
                    abort = true;
                }
            }
        }

        // First-chunk fetch failed (aborted or timed out) — surface an error.
        let Some(first) = first_chunk else {
            cancel.cancel();
            self.metrics.inc_empty_stream();
            tracing::warn!(
                elapsed_ms = start.elapsed().as_millis() as u64,
                "stream closed before any content; terminating"
            );
            let err = serde_json::json!({
                "error": {
                    "message": "upstream returned an empty stream (no content, no finish event)",
                    "type": "upstream_empty"
                }
            });
            self.send_json(session, 502, &err).await?;
            self.metrics.stream_finished();
            return Ok(true);
        };

        // Write the 200 SSE header now — we have real content.
        let mut resp = pingora_http::ResponseHeader::build(200, None)?;
        resp.insert_header("content-type", "text/event-stream")?;
        resp.insert_header("cache-control", "no-cache")?;
        resp.insert_header("connection", "keep-alive")?;
        let header_ok = session
            .write_response_header(Box::new(resp), false)
            .await
            .is_ok();

        async fn write_chunk(
            session: &mut Session,
            line: &str,
        ) -> std::result::Result<usize, pingora_error::BError> {
            let len = line.len();
            session
                .write_response_body(Some(Bytes::from(line.to_string())), false)
                .await?;
            Ok(len)
        }

        let mut chunks = 0u32;
        let mut bytes_out = 0usize;
        let mut client_gone = !header_ok;
        if !client_gone {
            // First chunk (rendered for the active frontend dialect).
            let first = match anthropic_renderer {
                Some(ref mut r) => r.feed(&first).join(""),
                None => first,
            };
            if !first.is_empty() {
                match write_chunk(session, &first).await {
                    Ok(len) => {
                        chunks += 1;
                        bytes_out += len;
                    }
                    Err(e) => {
                        if is_client_disconnect(&e) {
                            tracing::warn!("client disconnected; aborting stream");
                            self.metrics.inc_client_disconnect();
                        } else {
                            tracing::warn!(error = %e, "non-disconnect write error; aborting stream");
                        }
                        client_gone = true;
                    }
                }
            }

            // Stream the rest.
            while !client_gone {
                match tokio::time::timeout(idle_timeout, rx.recv()).await {
                    Ok(Some(Ok(line))) => {
                        // Render for the active frontend dialect; the
                        // Anthropic renderer may emit zero frames for a
                        // given upstream chunk.
                        let rendered = match anthropic_renderer {
                            Some(ref mut r) => r.feed(&line).join(""),
                            None => line,
                        };
                        if rendered.is_empty() {
                            continue;
                        }
                        match write_chunk(session, &rendered).await {
                            Ok(len) => {
                                chunks += 1;
                                bytes_out += len;
                            }
                            Err(e) => {
                                if is_client_disconnect(&e) {
                                    tracing::warn!("client disconnected; aborting stream");
                                    self.metrics.inc_client_disconnect();
                                } else {
                                    tracing::warn!(error=%e,"non-disconnect write error; aborting stream");
                                }
                                client_gone = true;
                            }
                        }
                    }
                    Ok(Some(Err(e))) => {
                        tracing::error!(error = %e, "stream error");
                        self.metrics.inc_error();
                        abort = true;
                        break;
                    }
                    Ok(None) => break,
                    Err(_) => {
                        self.metrics.inc_upstream_timeout();
                        tracing::error!(
                            idle_secs = idle_timeout.as_secs(),
                            "stream idle timeout; aborting"
                        );
                        abort = true;
                        break;
                    }
                }
            }
        }

        if client_gone || abort {
            cancel.cancel();
        }
        if !client_gone {
            let _ = session.write_response_body(None, true).await;
        }
        self.metrics.inc_chunks(chunks);
        self.metrics.inc_bytes_out(bytes_out);
        self.metrics.stream_finished();

        tracing::info!(
            elapsed_ms = start.elapsed().as_millis() as u64,
            chunks = chunks,
            "completed (stream)"
        );
        Ok(true)
    }

    /// Ollama-native model listing (`GET /api/tags`).
    async fn handle_ollama_tags(&self, session: &mut Session) -> PingoraResult<()> {
        let router = self.upstream_client.router.get().await;
        let mut names: Vec<Value> = Vec::new();
        for m in &router.models {
            if let Some(id) = m.get("id").and_then(|v| v.as_str()) {
                names.push(serde_json::json!({
                    "name": id,
                    "model": id,
                    "modified_at": "2024-01-01T00:00:00Z",
                    "size": 0,
                    "digest": "",
                }));
            }
        }
        let catalog = get_model_catalog();
        for id in catalog.keys() {
            names.push(serde_json::json!({
                "name": id.as_ref(),
                "model": id.as_ref(),
                "modified_at": "2024-01-01T00:00:00Z",
                "size": 0,
                "digest": "",
            }));
        }
        self.send_json(session, 200, &serde_json::json!({ "models": names }))
            .await?;
        Ok(())
    }

    async fn handle_models(&self, session: &mut Session) -> PingoraResult<()> {
        let mut seen = std::collections::HashSet::new();
        let mut models: Vec<serde_json::Value> = Vec::new();

        // Declared provider models first (opencode-style providers map).
        let router = self.upstream_client.router.get().await;
        for m in &router.models {
            if let Some(id) = m.get("id").and_then(|v| v.as_str()) {
                seen.insert(id.to_string());
            }
            models.push(m.clone());
        }

        // Bundled catalog entries not shadowed by a provider declaration.
        let catalog = get_model_catalog();
        for (id, meta) in catalog {
            if seen.insert(id.as_ref().to_string()) {
                models.push(serde_json::json!({
                    "id": id.as_ref(),
                    "object": "model",
                    "created": 0,
                    "owned_by": meta.provider.as_ref(),
                    "name": meta.name,
                    "reasoning": meta.reasoning,
                    "efforts": meta.efforts.iter().map(|e| e.as_str()).collect::<Vec<_>>(),
                    "context_window": meta.context_window.as_u64(),
                }));
            }
        }

        let response = serde_json::json!({
            "object": "list",
            "data": models,
        });

        self.send_json(session, 200, &response).await
    }

    async fn handle_health(&self, session: &mut Session) -> PingoraResult<()> {
        let catalog = get_model_catalog();

        // /health is intentionally unauthenticated (for monitors/scrapers), so
        // it must NOT leak the auth directory path, auth method, credential
        // validity, or the upstream URL. Only operational status is exposed.
        let response = serde_json::json!({
            "status": "ok",
            "models": catalog.len(),
            "default_model": self.config.default_model,
        });

        self.send_json(session, 200, &response).await
    }

    async fn handle_metrics(&self, session: &mut Session) -> PingoraResult<()> {
        let body = self.metrics.render();
        self.send_text(session, 200, "text/plain; version=0.0.4", &body)
            .await
    }

    /// Send a JSON response — serializes directly to bytes (no double serialization).
    async fn send_json(
        &self,
        session: &mut Session,
        status: u16,
        body: &serde_json::Value,
    ) -> PingoraResult<()> {
        let mut resp = pingora_http::ResponseHeader::build(status, None)?;
        resp.insert_header("content-type", "application/json")?;
        if let Some(ref origin) = self.config.cors_origin {
            // F-4: Validate origin is a valid URL scheme
            if origin == "*" || origin.starts_with("http://") || origin.starts_with("https://") {
                resp.insert_header("access-control-allow-origin", origin.as_str())?;
                resp.insert_header("access-control-allow-methods", "GET, POST, OPTIONS")?;
                resp.insert_header(
                    "access-control-allow-headers",
                    "Content-Type, Authorization",
                )?;
            }
        }
        session.write_response_header(Box::new(resp), false).await?;

        // Serialize directly to bytes — avoid intermediate Value serialization
        let bytes = serde_json::to_vec(body)
            .map_err(|e| Error::because(ErrorType::InternalError, "json serialize", e))?;
        session
            .write_response_body(Some(Bytes::from(bytes)), true)
            .await?;
        Ok(())
    }

    /// Send a plain-text response.
    async fn send_text(
        &self,
        session: &mut Session,
        status: u16,
        content_type: &str,
        body: &str,
    ) -> PingoraResult<()> {
        let mut resp = pingora_http::ResponseHeader::build(status, None)?;
        resp.insert_header("content-type", content_type)?;
        session.write_response_header(Box::new(resp), false).await?;
        session
            .write_response_body(Some(Bytes::from(body.as_bytes().to_vec())), true)
            .await?;
        Ok(())
    }
}

/// Constant-time string comparison. Pads both inputs to the same length
/// to avoid timing side-channels that could reveal token length.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    // Pad both to the same length to avoid length leak via timing.
    // Use the longer length so short tokens don't reveal their length
    // by comparing against zero-padded expected token.
    let max_len = a.len().max(b.len());
    let mut diff: u8 = 0;
    for i in 0..max_len {
        let a_val = a.get(i).copied().unwrap_or(0);
        let b_val = b.get(i).copied().unwrap_or(0);
        diff |= a_val ^ b_val;
    }
    // Also check that lengths match (constant-time)
    diff |= (a.len() ^ b.len()) as u8;
    diff == 0
}

/// Mask an API key for logging, showing only first and last 4 characters.
fn mask_api_key(key: &str) -> String {
    if key.len() <= 8 {
        return "*".repeat(key.len());
    }
    let prefix = &key[..4];
    let suffix = &key[key.len() - 4..];
    let masked_len = key.len() - 8;
    format!("{prefix}{}{suffix}", "*".repeat(masked_len))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constant_time_eq_matches() {
        assert!(constant_time_eq(b"secret-token", b"secret-token"));
    }

    #[test]
    fn test_constant_time_eq_mismatch() {
        assert!(!constant_time_eq(b"secret-token", b"secret-tokee"));
        assert!(!constant_time_eq(b"secret-token", b"secret-tokenX"));
        assert!(!constant_time_eq(b"secret-token", b""));
        assert!(!constant_time_eq(b"", b"secret-token"));
    }

    #[test]
    fn test_constant_time_eq_empty() {
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn test_constant_time_eq_different_lengths() {
        // F-3 fix: different lengths should still return false
        assert!(!constant_time_eq(b"short", b"longer-token"));
        assert!(!constant_time_eq(b"a", b"ab"));
        assert!(!constant_time_eq(b"ab", b"a"));
    }

    #[test]
    fn test_constant_time_eq_same_length_different_content() {
        assert!(!constant_time_eq(b"abcdef", b"abcdeg"));
        assert!(!constant_time_eq(b"000000", b"000001"));
    }
}
