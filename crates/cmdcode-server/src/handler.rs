use async_trait::async_trait;
use bytes::Bytes;
use pingora_core::upstreams::peer::HttpPeer;
use pingora_error::{Error, ErrorType, Result as PingoraResult};
use pingora_http::RequestHeader;
use pingora_proxy::{ProxyHttp, Session};
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
                // Continue to upstream
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
        let api_key = if let Some(ref expected) = self.config.incoming_token {
            let provided = session
                .req_header()
                .headers
                .get(http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .unwrap_or("");
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

        let body: ChatCompletionRequest = match serde_json::from_slice(&body_bytes) {
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
        };

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
                self.metrics.inc_bytes_out(completion.to_string().len());
                self.send_json(session, 200, &completion).await?;
                Ok(true)
            }
            Ok(upstream::UpstreamResponse::Sse { rx, cancel }) => {
                self.handle_sse_stream(session, rx, cancel, &model, &body, effort, start)
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

        let mut chunks = 0u32;
        let mut bytes_out = 0usize;
        let mut client_gone = !header_ok;
        if !client_gone {
            // First chunk.
            let first_len = first.len();
            if let Err(e) = session
                .write_response_body(Some(Bytes::from(first)), false)
                .await
            {
                if is_client_disconnect(&e) {
                    tracing::warn!("client disconnected; aborting stream");
                    self.metrics.inc_client_disconnect();
                } else {
                    tracing::warn!(error = %e, "non-disconnect write error; aborting stream");
                }
                client_gone = true;
            } else {
                chunks += 1;
                bytes_out += first_len;
            }

            // Stream the rest.
            while !client_gone {
                match tokio::time::timeout(idle_timeout, rx.recv()).await {
                    Ok(Some(Ok(line))) => {
                        if let Err(e) = session
                            .write_response_body(Some(Bytes::from(line.clone())), false)
                            .await
                        {
                            if is_client_disconnect(&e) {
                                tracing::warn!("client disconnected; aborting stream");
                                self.metrics.inc_client_disconnect();
                            } else {
                                tracing::warn!(error=%e,"non-disconnect write error; aborting stream");
                            }
                            client_gone = true;
                        } else {
                            chunks += 1;
                            bytes_out += line.len();
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

    async fn handle_models(&self, session: &mut Session) -> PingoraResult<()> {
        let catalog = get_model_catalog();
        let models: Vec<serde_json::Value> = catalog
            .iter()
            .map(|(id, meta)| {
                serde_json::json!({
                    "id": id.as_ref(),
                    "object": "model",
                    "created": 0,
                    "owned_by": meta.provider.as_ref(),
                    "name": meta.name,
                    "reasoning": meta.reasoning,
                    "efforts": meta.efforts.iter().map(|e| e.as_str()).collect::<Vec<_>>(),
                    "context_window": meta.context_window.as_u64(),
                })
            })
            .collect();

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
