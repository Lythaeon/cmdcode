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

use crate::upstream::{self, UpstreamClient};

pub struct CommandCodeProxy {
    pub config: Arc<ProxyConfig>,
    pub auth: Arc<AuthManager>,
    pub upstream_client: Arc<UpstreamClient>,
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
            "/v1/chat/completions" | "/chat/completions" if method == http::Method::POST => {
                // Continue to upstream
            }
            _ => {
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

        // Read and parse body with size limit.
        // read_request_body() returns at most one 64KB chunk per call —
        // loop until None to collect the full body.
        let mut body_bytes = Vec::new();
        while let Some(chunk) = session.read_request_body().await? {
            body_bytes.extend_from_slice(&chunk);
            if body_bytes.len() > self.config.max_body_size {
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
                self.send_json(session, 200, &completion).await?;
                Ok(true)
            }
            Ok(upstream::UpstreamResponse::Sse { mut rx }) => {
                tracing::info!(
                    request_id = %ctx.request_id.as_str(),
                    "starting stream"
                );

                let mut resp = pingora_http::ResponseHeader::build(200, None)?;
                resp.insert_header("content-type", "text/event-stream")?;
                resp.insert_header("cache-control", "no-cache")?;
                resp.insert_header("connection", "keep-alive")?;
                session.write_response_header(Box::new(resp), false).await?;

                let mut chunks = 0u32;
                while let Some(chunk) = rx.recv().await {
                    match chunk {
                        Ok(line) => {
                            session
                                .write_response_body(Some(Bytes::from(line)), false)
                                .await?;
                            chunks += 1;
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "stream error");
                            break;
                        }
                    }
                }

                session.write_response_body(None, true).await?;

                tracing::info!(
                    request_id = %ctx.request_id.as_str(),
                    elapsed_ms = start.elapsed().as_millis() as u64,
                    chunks = chunks,
                    "completed (stream)"
                );
                Ok(true)
            }
            Err(e) => {
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
}
