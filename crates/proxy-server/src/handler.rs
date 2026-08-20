use async_trait::async_trait;
use bytes::Bytes;
use pingora_core::upstreams::peer::HttpPeer;
use pingora_error::{Error, ErrorType, Result as PingoraResult};
use pingora_http::RequestHeader;
use pingora_proxy::{ProxyHttp, Session};
use std::sync::Arc;
use std::time::Instant;

use proxy_core::auth::AuthManager;
use proxy_core::config::ProxyConfig;
use proxy_core::model_catalog::get_model_catalog;
use proxy_core::types::RequestId;
use proxy_core::wire_format::ChatCompletionRequest;

use crate::metrics::Metrics;
use crate::upstream::{self, UpstreamClient};

pub struct CommandCodeProxy {
    pub config: Arc<ProxyConfig>,
    pub auth: Arc<AuthManager>,
    pub upstream_client: Arc<UpstreamClient>,
    pub metrics: Arc<Metrics>,
}

pub struct RequestCtx {
    pub request_id: RequestId,
    pub start: Instant,
}

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
            .split("://").nth(1)
            .and_then(|s| s.split(':').next())
            .unwrap_or("api.commandcode.ai")
            .to_string();
        let tls = upstream_url.starts_with("https");

        let peer = HttpPeer::new((host.clone(), self.upstream_client.config.upstream_url
            .split(':').nth(2).and_then(|s| s.split('/').next())
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(if tls { 443 } else { 80 })), tls, host);
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
                resp.insert_header("access-control-allow-origin", origin.as_str())?;
                resp.insert_header("access-control-allow-methods", "GET, POST, OPTIONS")?;
                resp.insert_header("access-control-allow-headers", "Content-Type, Authorization")?;
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
        if let Some(ref expected) = self.config.incoming_token {
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
        let model_id_str = body
            .model
            .as_deref()
            .unwrap_or(&self.config.default_model);
        let (model, effort) = proxy_core::types::parse_model_and_effort(model_id_str);
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
        let result = self.upstream_client.forward_request(&model, &body, effort).await;

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
            Ok(upstream::UpstreamResponse::Sse { mut rx, cancel }) => {
                tracing::info!(
                    request_id = %ctx.request_id.as_str(),
                    elapsed_ms = start.elapsed().as_millis() as u64,
                    "starting stream"
                );
                self.metrics.stream_started();

                let mut resp = pingora_http::ResponseHeader::build(200, None)?;
                resp.insert_header("content-type", "text/event-stream")?;
                resp.insert_header("cache-control", "no-cache")?;
                resp.insert_header("connection", "keep-alive")?;
                session.write_response_header(Box::new(resp), false).await?;

                let idle_timeout = std::time::Duration::from_secs(self.config.stream_idle_timeout_secs);
                let mut chunks = 0u32;
                let mut bytes_out = 0usize;
                let mut client_gone = false;
                let mut abort = false;
                loop {
                    let recv = tokio::time::timeout(idle_timeout, rx.recv()).await;
                    match recv {
                        Ok(Some(Ok(line))) => {
                            if let Err(e) = session.write_response_body(Some(Bytes::from(line.clone())), false).await {
                                if is_client_disconnect(&e) {
                                    tracing::warn!(request_id = %ctx.request_id.as_str(), "client disconnected; aborting stream");
                                    self.metrics.inc_client_disconnect();
                                    client_gone = true;
                                } else {
                                    return Err(e);
                                }
                                break;
                            }
                            chunks += 1;
                            bytes_out += line.len();
                        }
                        Ok(Some(Err(e))) => {
                            tracing::error!(request_id = %ctx.request_id.as_str(), error = %e, "stream error");
                            self.metrics.inc_error();
                            abort = true;
                            break;
                        }
                        Ok(None) => break,
                        Err(_) => {
                            self.metrics.inc_upstream_timeout();
                            tracing::error!(
                                request_id = %ctx.request_id.as_str(),
                                idle_secs = idle_timeout.as_secs(),
                                "stream idle timeout; aborting"
                            );
                            abort = true;
                            break;
                        }
                    }
                }

                // If we are not ending the stream naturally (the upstream task
                // already sent [DONE] and closed the channel), signal its
                // cancellation so it drops the reqwest connection and releases
                // the concurrency permit immediately instead of waiting on
                // Command Code to produce more data.
                if client_gone || abort {
                    cancel.notify_waiters();
                }

                if !client_gone {
                    session.write_response_body(None, true).await?;
                }
                self.metrics.inc_chunks(chunks);
                self.metrics.inc_bytes_out(bytes_out);
                self.metrics.stream_finished();

                tracing::info!(
                    request_id = %ctx.request_id.as_str(),
                    elapsed_ms = start.elapsed().as_millis() as u64,
                    chunks = chunks,
                    "completed (stream)"
                );
                Ok(true)
            }
            Err(e) => {
                self.metrics.inc_error();
                if matches!(
                    e,
                    proxy_core::error::UpstreamError::Timeout { .. }
                ) {
                    self.metrics.inc_upstream_timeout();
                }
                tracing::error!(error = %e, "upstream error");
                let status = match &e {
                    proxy_core::error::UpstreamError::ConnectionRefused { .. } => 502,
                    proxy_core::error::UpstreamError::ConnectionReset => 502,
                    proxy_core::error::UpstreamError::Timeout { .. } => 504,
                    proxy_core::error::UpstreamError::HttpError { status, .. } => *status,
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
        let auth_health = self.auth.health_check().await;

        let response = serde_json::json!({
            "status": "ok",
            "upstream": self.config.upstream_url,
            "models": catalog.len(),
            "default_model": self.config.default_model,
            "auth": auth_health,
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
            resp.insert_header("access-control-allow-origin", origin.as_str())?;
            resp.insert_header("access-control-allow-methods", "GET, POST, OPTIONS")?;
            resp.insert_header("access-control-allow-headers", "Content-Type, Authorization")?;
        }
        session.write_response_header(Box::new(resp), false).await?;

        // Serialize directly to bytes — avoid intermediate Value serialization
        let bytes = serde_json::to_vec(body)
            .map_err(|e| Error::because(ErrorType::InternalError, "json serialize", e))?;
        session.write_response_body(Some(Bytes::from(bytes)), true).await?;
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

/// Constant-time string comparison. On length mismatch this still leaks the
/// length, which is inherent to fixed-length bearer-token checks.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
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
}
