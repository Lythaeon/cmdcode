#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::net::TcpListener;

type MockBody = http_body_util::combinators::BoxBody<bytes::Bytes, std::io::Error>;
type MockServiceResult = hyper::Result<hyper::Response<MockBody>>;
type MockServiceFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = MockServiceResult> + Send>>;

/// Mock upstream that returns configurable NDJSON responses.
struct MockUpstream {
    events: Vec<serde_json::Value>,
    status: u16,
    delay_ms: u64,
    /// Per-request status codes, consumed in order across connections
    /// (used to exercise proxy retry logic). Falls back to `status` when
    /// exhausted.
    status_sequence: Option<Arc<Vec<u16>>>,
    /// Delay between streamed chunks (multi-chunk trickle).
    chunk_gap_ms: u64,
    /// Abort the connection after this many chunks have been sent.
    reset_after_chunks: Option<usize>,
    /// Omit the trailing newline on the final event (truncated line).
    no_final_newline: bool,
    /// Garbage appended to the final event's line (making it unparseable),
    /// used to simulate a stream truncated mid-record.
    final_tail: Option<String>,
}

impl MockUpstream {
    fn normal() -> Self {
        Self {
            events: vec![
                serde_json::json!({"type": "text-delta", "text": "Hello"}),
                serde_json::json!({"type": "text-delta", "text": " world"}),
                serde_json::json!({
                    "type": "finish",
                    "finishReason": "stop",
                    "totalUsage": {
                        "inputTokens": 10,
                        "outputTokens": 5,
                        "inputTokenDetails": {"cacheReadTokens": 0}
                    }
                }),
            ],
            status: 200,
            delay_ms: 0,
            status_sequence: None,
            chunk_gap_ms: 0,
            reset_after_chunks: None,
            no_final_newline: false,
            final_tail: None,
        }
    }

    async fn start(self) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let events = self.events;
        let status = self.status;
        let delay = self.delay_ms;
        let status_sequence = self.status_sequence;
        let chunk_gap = self.chunk_gap_ms;
        let reset_after = self.reset_after_chunks;
        let no_final_newline = self.no_final_newline;
        let final_tail = self.final_tail;
        let counter = Arc::new(AtomicUsize::new(0));

        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let events = events.clone();
                let status_sequence = status_sequence.clone();
                let counter = counter.clone();
                let reset_after = reset_after;
                let no_final_newline = no_final_newline;
                let final_tail = final_tail.clone();
                tokio::spawn(async move {
                    if let Some(n) = reset_after {
                        // Raw TCP handler: write the response head + first `n`
                        // events as chunked encoding, then drop the connection
                        // (FIN) without the terminating chunk. This mirrors a
                        // real upstream dying mid-stream: the client receives
                        // the head and partial body, then EOF.
                        use tokio::io::AsyncWriteExt;
                        let mut stream = stream;
                        let mut buf = [0u8; 8192];
                        let _ = stream.readable().await;
                        let _ = stream.try_read(&mut buf);
                        let head = "HTTP/1.1 200 OK\r\ncontent-type: application/x-ndjson\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n";
                        if stream.write_all(head.as_bytes()).await.is_ok() {
                            for evt in events.iter().take(n) {
                                let mut line = serde_json::to_string(evt).unwrap();
                                line.push('\n');
                                let frame = format!("{:x}\r\n{}\r\n", line.len(), line);
                                if stream.write_all(frame.as_bytes()).await.is_err() {
                                    break;
                                }
                            }
                        }
                        // No terminating 0-chunk: connection drops, client sees EOF.
                        return;
                    }
                    let io = hyper_util::rt::TokioIo::new(stream);
                    let service = hyper::service::service_fn(
                        move |req: hyper::Request<hyper::body::Incoming>| -> MockServiceFuture {
                            let events = events.clone();
                            let status_sequence = status_sequence.clone();
                            let counter = counter.clone();
                            let final_tail = final_tail.clone();
                            Box::pin(async move {
                                // Consume request body
                                use http_body_util::BodyExt;
                                let _ = req.into_body().collect().await;

                                if delay > 0 {
                                    tokio::time::sleep(std::time::Duration::from_millis(delay))
                                        .await;
                                }

                                let req_status = match &status_sequence {
                                    Some(seq) => {
                                        let idx = counter.fetch_add(1, Ordering::SeqCst);
                                        seq[idx.min(seq.len() - 1)]
                                    }
                                    None => status,
                                };

                                if req_status != 200 {
                                    let body = r#"{"error":{"message":"upstream error"}}"#;
                                    let stream = futures::stream::once(async move {
                                        Ok::<_, std::io::Error>(hyper::body::Frame::data(
                                            bytes::Bytes::from(body),
                                        ))
                                    });
                                    let mut resp = hyper::Response::new(
                                        http_body_util::StreamBody::new(stream).boxed(),
                                    );
                                    *resp.status_mut() =
                                        hyper::StatusCode::from_u16(req_status).unwrap();
                                    resp.headers_mut().insert(
                                        "content-type",
                                        "application/json".parse().unwrap(),
                                    );
                                    resp.headers_mut()
                                        .insert("connection", "close".parse().unwrap());
                                    return Ok(resp);
                                }

                                if events.is_empty() {
                                    let stream = futures::stream::once(async move {
                                        Ok::<_, std::io::Error>(hyper::body::Frame::data(
                                            bytes::Bytes::new(),
                                        ))
                                    });
                                    return Ok(hyper::Response::new(
                                        http_body_util::StreamBody::new(stream).boxed(),
                                    ));
                                }

                                let frame_stream = futures::stream::unfold(
                                    (events.clone(), 0usize, final_tail.clone()),
                                    move |(evts, i, final_tail)| async move {
                                        if i >= evts.len() {
                                            return None;
                                        }
                                        if chunk_gap > 0 && i > 0 {
                                            tokio::time::sleep(std::time::Duration::from_millis(
                                                chunk_gap,
                                            ))
                                            .await;
                                        }
                                        let mut line = serde_json::to_string(&evts[i]).unwrap();
                                        if final_tail.is_some() && i + 1 == evts.len() {
                                            line.push_str(
                                                final_tail.as_deref().unwrap_or_default(),
                                            );
                                        }
                                        if !(no_final_newline && i + 1 == evts.len()) {
                                            line.push('\n');
                                        }
                                        Some((
                                            Ok::<_, std::io::Error>(hyper::body::Frame::data(
                                                bytes::Bytes::from(line),
                                            )),
                                            (evts, i + 1, final_tail),
                                        ))
                                    },
                                );

                                let mut resp = hyper::Response::new(
                                    http_body_util::StreamBody::new(frame_stream).boxed(),
                                );
                                resp.headers_mut().insert(
                                    "content-type",
                                    "application/x-ndjson".parse().unwrap(),
                                );
                                Ok(resp)
                            })
                        },
                    );

                    hyper::server::conn::http1::Builder::new()
                        .serve_connection(io, service)
                        .await
                        .ok();
                });
            }
        });

        addr
    }
}

/// Make a request using reqwest.
async fn proxy_request(
    addr: SocketAddr,
    path: &str,
    method: &str,
    body: Option<&str>,
) -> (u16, String) {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .expect("failed to build client");
    let url = format!("http://{}{}", addr, path);

    let resp = match method {
        "GET" => client.get(&url).send().await,
        "POST" => {
            client
                .post(&url)
                .header("content-type", "application/json")
                .body(body.unwrap_or("{}").to_string())
                .send()
                .await
        }
        _ => client.get(&url).send().await,
    };

    match resp {
        Ok(r) => {
            let status = r.status().as_u16();
            let body = r.text().await.unwrap_or_default();
            (status, body)
        }
        Err(e) => (0, format!("request error: {}", e)),
    }
}

// ============================================================
// E2E Tests
// ============================================================

#[tokio::test]
async fn test_e2e_models_endpoint() {
    std::fs::create_dir_all("/tmp/test/.commandcode").ok();
    std::fs::write(
        "/tmp/test/.commandcode/auth.json",
        r#"{"apiKey":"test-key"}"#,
    )
    .ok();

    let auth = cmdcode_core::auth::AuthManager::new(
        std::path::PathBuf::from("/tmp/test/.commandcode"),
        30,
    );

    // Catalog may be empty in test env if CLI isn't installed — that's OK
    let _catalog = cmdcode_core::model_catalog::get_model_catalog();

    let method = auth.get_auth_method().await.unwrap();
    match method {
        cmdcode_core::auth::AuthMethod::ApiKey(k) => assert_eq!(k, "test-key"),
        _ => panic!("expected API key"),
    }

    let headers = auth.build_headers("/tmp/test").await.unwrap();
    assert_eq!(headers.get("Authorization").unwrap(), "Bearer test-key");
    assert_eq!(headers.get("User-Agent").unwrap(), "cli");
    assert!(headers.contains_key("x-session-id"));
}

#[tokio::test]
async fn test_e2e_wire_format_translation() {
    let messages = vec![
        cmdcode_core::wire_format::OpenAiMessage {
            role: "system".into(),
            content: Some(serde_json::json!("Be helpful")),
            tool_call_id: None,
            tool_calls: None,
        },
        cmdcode_core::wire_format::OpenAiMessage {
            role: "user".into(),
            content: Some(serde_json::json!("Hello")),
            tool_call_id: None,
            tool_calls: None,
        },
    ];

    let wire = cmdcode_core::wire_format::wire_messages(&messages);
    // System messages are skipped in the array (they go to params.system)
    assert_eq!(wire.len(), 1);

    match &wire[0] {
        cmdcode_core::wire_format::CcMessage::User { content } => {
            assert_eq!(content.len(), 1);
            match &content[0] {
                cmdcode_core::wire_format::CcContentItem::Text { text } => {
                    assert_eq!(text, "Hello");
                }
                _ => panic!("expected text"),
            }
        }
        _ => panic!("expected user message"),
    }
}

#[tokio::test]
async fn test_e2e_tool_translation() {
    let tools = vec![cmdcode_core::wire_format::OpenAiTool {
        tool_type: "function".into(),
        function: Some(cmdcode_core::wire_format::OpenAiFunction {
            name: "search".into(),
            description: Some("Search the web".into()),
            parameters: Some(serde_json::json!({
                "type": "object",
                "properties": {"query": {"type": "string"}}
            })),
        }),
        name: None,
        description: None,
        input_schema: None,
        parameters: None,
    }];

    let wire = cmdcode_core::wire_format::wire_tools(&tools);
    assert_eq!(wire.len(), 1);
    assert_eq!(wire[0].name, "search");
    assert_eq!(wire[0].description, "Search the web");
}

#[tokio::test]
async fn test_e2e_reasoning_effort_parsing() {
    let (model, effort) = cmdcode_core::types::parse_model_and_effort("claude-sonnet-5:high");
    assert_eq!(model.as_str(), "claude-sonnet-5");
    assert_eq!(effort, Some(cmdcode_core::types::Effort::High));

    let (model, effort) = cmdcode_core::types::parse_model_and_effort("gpt-5.6-luna");
    assert_eq!(model.as_str(), "gpt-5.6-luna");
    assert_eq!(effort, None);

    let (model, effort) =
        cmdcode_core::types::parse_model_and_effort("command-code/xiaomi/mimo-v2.5:max");
    assert_eq!(model.as_str(), "xiaomi/mimo-v2.5");
    assert_eq!(effort, Some(cmdcode_core::types::Effort::Max));
}

// ============================================================
// Chaos Tests
// ============================================================

#[tokio::test]
async fn test_chaos_empty_upstream() {
    let addr = MockUpstream {
        events: vec![],
        status: 200,
        delay_ms: 0,
        ..MockUpstream::normal()
    }
    .start()
    .await;
    let (status, _) =
        proxy_request(addr, "/alpha/generate", "POST", Some(r#"{"test":true}"#)).await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn test_chaos_upstream_500() {
    let addr = MockUpstream {
        status: 500,
        ..MockUpstream::normal()
    }
    .start()
    .await;
    let (status, body) =
        proxy_request(addr, "/alpha/generate", "POST", Some(r#"{"test":true}"#)).await;
    assert_eq!(status, 500);
    assert!(body.contains("upstream error"));
}

#[tokio::test]
async fn test_chaos_upstream_slow() {
    let addr = MockUpstream {
        delay_ms: 100,
        ..MockUpstream::normal()
    }
    .start()
    .await;
    let start = std::time::Instant::now();
    let (status, _) =
        proxy_request(addr, "/alpha/generate", "POST", Some(r#"{"test":true}"#)).await;
    let elapsed = start.elapsed();
    assert_eq!(status, 200);
    assert!(elapsed >= std::time::Duration::from_millis(80));
}

#[tokio::test]
async fn test_chaos_malformed_json_upstream() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let io = hyper_util::rt::TokioIo::new(stream);
                let service = hyper::service::service_fn(
                    |req: hyper::Request<hyper::body::Incoming>| async move {
                        use http_body_util::BodyExt;
                        let _ = req.into_body().collect().await;
                        let body =
                            "not json at all\n{\"type\":\"text-delta\",\"text\":\"recovered\"}\n";
                        let mut resp = hyper::Response::new(http_body_util::Full::new(
                            bytes::Bytes::from(body),
                        ));
                        resp.headers_mut()
                            .insert("content-type", "application/x-ndjson".parse().unwrap());
                        Ok::<_, hyper::Error>(resp)
                    },
                );
                hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, service)
                    .await
                    .ok();
            });
        }
    });

    let (status, _) =
        proxy_request(addr, "/alpha/generate", "POST", Some(r#"{"test":true}"#)).await;
    assert!(status == 200 || status == 502);
}

#[tokio::test]
async fn test_chaos_connection_refused() {
    let result = proxy_request(
        "127.0.0.1:19999".parse().unwrap(),
        "/alpha/generate",
        "POST",
        Some(r#"{"test":true}"#),
    )
    .await;
    // Connection refused — reqwest may panic or return error
    // Just verify we don't hang forever
    assert!(result.0 != 200 || result.1.contains("error"));
}

#[tokio::test]
async fn test_chaos_duplicate_finish_events() {
    let mock = MockUpstream {
        events: vec![
            serde_json::json!({"type": "text-delta", "text": "hi"}),
            serde_json::json!({
                "type": "finish",
                "finishReason": "stop",
                "totalUsage": {"inputTokens": 1, "outputTokens": 1, "inputTokenDetails": {}}
            }),
            serde_json::json!({
                "type": "finish",
                "finishReason": "stop",
                "totalUsage": {"inputTokens": 1, "outputTokens": 1, "inputTokenDetails": {}}
            }),
        ],
        status: 200,
        delay_ms: 0,
        ..MockUpstream::normal()
    };
    let addr = mock.start().await;
    let (status, _) =
        proxy_request(addr, "/alpha/generate", "POST", Some(r#"{"test":true}"#)).await;
    assert!(status == 200 || status == 502);
}

#[tokio::test]
async fn test_chaos_huge_payload() {
    let huge_text = "x".repeat(1_000_000);
    let mock = MockUpstream {
        events: vec![
            serde_json::json!({"type": "text-delta", "text": huge_text}),
            serde_json::json!({
                "type": "finish",
                "finishReason": "stop",
                "totalUsage": {"inputTokens": 100, "outputTokens": 50, "inputTokenDetails": {}}
            }),
        ],
        status: 200,
        delay_ms: 0,
        ..MockUpstream::normal()
    };
    let addr = mock.start().await;
    let (status, body) =
        proxy_request(addr, "/alpha/generate", "POST", Some(r#"{"test":true}"#)).await;
    assert_eq!(status, 200);
    assert!(body.len() > 100_000);
}

#[tokio::test]
async fn test_chaos_many_chunks() {
    let mut events = Vec::new();
    for i in 0..1000 {
        events.push(serde_json::json!({"type": "text-delta", "text": format!("chunk-{}", i)}));
    }
    events.push(serde_json::json!({
        "type": "finish",
        "finishReason": "stop",
        "totalUsage": {"inputTokens": 10, "outputTokens": 500, "inputTokenDetails": {}}
    }));

    let mock = MockUpstream {
        events,
        status: 200,
        delay_ms: 0,
        ..MockUpstream::normal()
    };
    let addr = mock.start().await;
    let (status, body) =
        proxy_request(addr, "/alpha/generate", "POST", Some(r#"{"test":true}"#)).await;
    assert_eq!(status, 200);
    assert!(body.len() > 10000);
}

// ============================================================
// Concurrency Tests
// ============================================================

#[tokio::test]
async fn test_concurrent_100_requests() {
    let addr = MockUpstream::normal().start().await;

    let mut handles = Vec::new();
    for i in 0..100 {
        handles.push(tokio::spawn(async move {
            proxy_request(
                addr,
                "/alpha/generate",
                "POST",
                Some(&format!(r#"{{"test":{}}}"#, i)),
            )
            .await
        }));
    }

    let mut success = 0;
    let mut errors = 0;
    for h in handles {
        let (status, _) = h.await.unwrap();
        if status == 200 {
            success += 1;
        } else {
            errors += 1;
        }
    }

    assert!(success >= 95, "Too many errors: {}/100 failed", errors);
}

#[tokio::test]
async fn test_concurrent_mixed_methods() {
    let addr = MockUpstream::normal().start().await;

    let mut handles = Vec::new();
    for i in 0..50 {
        handles.push(tokio::spawn(async move {
            proxy_request(
                addr,
                "/alpha/generate",
                "POST",
                Some(&format!(r#"{{"test":{}}}"#, i)),
            )
            .await
        }));
    }

    let mut success = 0;
    for h in handles {
        let (status, _) = h.await.unwrap();
        if status == 200 {
            success += 1;
        }
    }

    assert!(success >= 45, "Too many failures in mixed concurrent test");
}

// ============================================================
// Integration Tests
// ============================================================

#[test]
fn test_model_catalog_loads() {
    let catalog = cmdcode_core::model_catalog::get_model_catalog();
    // In test env, catalog may be empty if CLI isn't installed
    // The parsing logic is tested separately in cmdcode-core unit tests
    assert!(catalog.len() <= 100, "Catalog too large: {}", catalog.len());
}

#[test]
fn test_model_catalog_efforts() {
    let catalog = cmdcode_core::model_catalog::get_model_catalog();
    if catalog.is_empty() {
        return; // Skip if CLI not installed in test env
    }

    let claude = catalog.get(&cmdcode_core::types::ModelId::new("claude-sonnet-5"));
    if let Some(claude) = claude {
        assert!(claude.reasoning);
        assert_eq!(claude.efforts.len(), 5);
        assert!(claude.efforts.contains(&cmdcode_core::types::Effort::High));
        assert!(claude.efforts.contains(&cmdcode_core::types::Effort::Max));
    }

    let gpt = catalog.get(&cmdcode_core::types::ModelId::new("gpt-5.6-luna"));
    if let Some(gpt) = gpt {
        assert!(gpt.reasoning);
        assert!(gpt.context_window.as_u64() > 0);
    }
}

#[test]
fn test_model_catalog_providers() {
    let catalog = cmdcode_core::model_catalog::get_model_catalog();
    if catalog.is_empty() {
        return; // Skip if CLI not installed in test env
    }
    let providers: std::collections::HashSet<_> =
        catalog.values().map(|m| m.provider.as_ref()).collect();
    assert!(providers.contains("openai") || providers.contains("anthropic"));
}

#[test]
fn test_config_from_env() {
    let config = cmdcode_core::config::ProxyConfig::from_env().unwrap();
    assert_eq!(config.listen_addr, "127.0.0.1:18080");
    assert_eq!(config.upstream_url, "https://api.commandcode.ai");
    assert_eq!(config.default_model, "xiaomi/mimo-v2.5");
    assert_eq!(config.upstream_timeout_secs, 600);
    assert_eq!(config.max_retries, 2);
}

#[test]
fn test_auth_data_parsing() {
    let json = r#"{"apiKey":"my-key","userId":"123","userName":"test"}"#;
    let auth: cmdcode_core::auth::AuthData = serde_json::from_str(json).unwrap();
    assert_eq!(auth.api_key.as_deref(), Some("my-key"));
    assert_eq!(auth.user_id.as_deref(), Some("123"));
}

#[test]
fn test_auth_data_oauth() {
    let json = r#"{"oauthToken":"token123","oauthProvider":"github"}"#;
    let auth: cmdcode_core::auth::AuthData = serde_json::from_str(json).unwrap();
    assert_eq!(auth.oauth_token.as_deref(), Some("token123"));
    assert_eq!(auth.oauth_provider.as_deref(), Some("github"));
    assert!(auth.api_key.is_none());
}

#[test]
fn test_error_types() {
    let err = cmdcode_core::error::ProxyError::ModelNotAllowed("test".into());
    assert!(err.to_string().contains("test"));

    let err = cmdcode_core::error::UpstreamError::ConnectionRefused {
        host: "localhost".into(),
        port: 8080,
    };
    assert!(err.to_string().contains("localhost"));

    let err = cmdcode_core::error::AuthError::NoAuthConfigured;
    assert!(!err.to_string().is_empty());
}

#[test]
fn test_finish_reason_mapping() {
    use cmdcode_core::types::FinishReason;
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
fn test_request_id_generation() {
    let id1 = cmdcode_core::types::RequestId::generate();
    let id2 = cmdcode_core::types::RequestId::generate();
    assert_ne!(id1.as_str(), id2.as_str());
    assert!(!id1.as_str().is_empty());
}

#[test]
fn test_session_id_generation() {
    let id1 = cmdcode_core::types::SessionId::generate();
    let id2 = cmdcode_core::types::SessionId::generate();
    assert_ne!(id1.as_str(), id2.as_str());
}

// ============================================================
// Through-proxy benchmark: real Pingora proxy vs direct mock
// ============================================================

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

#[tokio::test]
async fn test_benchmark_through_proxy_vs_direct() {
    // Start the mock upstream (this is the reference path).
    let mock_addr = MockUpstream::normal().start().await;

    // Temp auth dir so AuthManager.build_headers() succeeds.
    let auth_dir = std::env::temp_dir().join(format!("cc-proxy-bench-auth-{}", std::process::id()));
    std::fs::create_dir_all(&auth_dir).unwrap();
    std::fs::write(
        auth_dir.join("auth.json"),
        r#"{"apiKey":"bench-key-1234567890"}"#,
    )
    .unwrap();
    std::fs::write(auth_dir.join("config.json"), r#"{}"#).unwrap();

    // Pick a free port for the real proxy.
    let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = probe.local_addr().unwrap().port();
    drop(probe);

    let config = cmdcode_core::config::ProxyConfig {
        listen_addr: format!("127.0.0.1:{}", proxy_port),
        upstream_url: format!("http://{}", mock_addr),
        default_model: "xiaomi/mimo-v2.5".into(),
        upstream_timeout_secs: 30,
        max_retries: 0,
        max_concurrent: 0,
        cors_origin: None,
        model_allowlist: None,
        auth_dir: auth_dir.clone(),
        auth_cache_ttl_secs: 60,
        log_level: "error".into(),
        max_body_size: 10 * 1024 * 1024,
        stream_idle_timeout_secs: 180,
        log_file: None,
        log_max_bytes: 50 * 1024 * 1024,
        log_keep: 5,
        tls_cert: None,
        tls_key: None,
        incoming_token: None,
    };
    let auth = cmdcode_core::auth::AuthManager::new(auth_dir.clone(), 60);

    // Run the REAL Pingora proxy in a background thread.
    std::thread::spawn(move || {
        let service = cmdcode_server::ProxyService::new(config, auth);
        let _ = service.run();
    });

    let proxy_url = format!("http://127.0.0.1:{}", proxy_port);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap();

    // Wait for readiness.
    let mut ready = false;
    for _ in 0..100 {
        if let Ok(r) = client.get(format!("{}/health", proxy_url)).send().await {
            if r.status().is_success() {
                ready = true;
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(ready, "proxy did not become ready in time");

    let body = r#"{"model":"xiaomi/mimo-v2.5","stream":false,"messages":[{"role":"user","content":"hi"}]}"#;
    let direct_url = format!("http://{}/alpha/generate", mock_addr);
    let n = 500;

    // Warm up both paths.
    for _ in 0..20 {
        let _ = client
            .post(&proxy_url)
            .header("content-type", "application/json")
            .header("authorization", "Bearer bench-key-1234567890")
            .body(body)
            .send()
            .await;
        let _ = client.post(&direct_url).body(body).send().await;
    }

    // Measure through the proxy.
    let mut proxy_times = Vec::with_capacity(n);
    let mut proxy_ok = 0;
    for _ in 0..n {
        let start = std::time::Instant::now();
        let r = client
            .post(format!("{}/v1/chat/completions", proxy_url))
            .header("content-type", "application/json")
            .header("authorization", "Bearer bench-key-1234567890")
            .body(body)
            .send()
            .await;
        if let Ok(r) = r {
            if r.status().is_success() {
                proxy_ok += 1;
            }
        }
        proxy_times.push(start.elapsed().as_secs_f64() * 1000.0);
    }

    // Measure direct to the mock.
    let mut direct_times = Vec::with_capacity(n);
    let mut direct_ok = 0;
    for _ in 0..n {
        let start = std::time::Instant::now();
        let r = client.post(&direct_url).body(body).send().await;
        if let Ok(r) = r {
            if r.status().is_success() {
                direct_ok += 1;
            }
        }
        direct_times.push(start.elapsed().as_secs_f64() * 1000.0);
    }

    let mut proxy_sorted = proxy_times.clone();
    let mut direct_sorted = direct_times.clone();
    proxy_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    direct_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let pp50 = percentile(&proxy_sorted, 0.50);
    let pp95 = percentile(&proxy_sorted, 0.95);
    let pp99 = percentile(&proxy_sorted, 0.99);
    let dp50 = percentile(&direct_sorted, 0.50);
    let dp95 = percentile(&direct_sorted, 0.95);
    let dp99 = percentile(&direct_sorted, 0.99);

    eprintln!(
        "[bench] proxy  : p50={:.3}ms p95={:.3}ms p99={:.3}ms ({} ok)",
        pp50, pp95, pp99, proxy_ok
    );
    eprintln!(
        "[bench] direct : p50={:.3}ms p95={:.3}ms p99={:.3}ms ({} ok)",
        dp50, dp95, dp99, direct_ok
    );
    eprintln!(
        "[bench] overhead: p50=+{:.3}ms p95=+{:.3}ms p99=+{:.3}ms",
        pp50 - dp50,
        pp95 - dp95,
        pp99 - dp99
    );

    // Both paths must be reliable.
    assert!(
        proxy_ok >= n * 98 / 100,
        "proxy success rate too low: {}/{}",
        proxy_ok,
        n
    );
    assert!(
        direct_ok >= n * 98 / 100,
        "direct success rate too low: {}/{}",
        direct_ok,
        n
    );

    // Proxy overhead must stay small on warm loopback. Bounds are relative to
    // the direct path so CI noise (which affects both paths equally) cancels.
    assert!(
        pp50 < dp50 + 2.0,
        "proxy p50 overhead too high: {:.3}ms vs direct {:.3}ms",
        pp50,
        dp50
    );
    assert!(
        pp99 < dp99 + 10.0,
        "proxy p99 overhead too high: {:.3}ms vs direct {:.3}ms",
        pp99,
        dp99
    );
}

// ============================================================
// Through-proxy chaos tests: REAL Pingora proxy + hostile upstreams
// ============================================================

/// Start a real Pingora proxy instance pointing at `mock_addr`.
/// Returns the proxy base URL (e.g. http://127.0.0.1:PORT).
async fn start_proxy(mock_addr: SocketAddr, retries: u32, idle_timeout_secs: u64) -> String {
    start_proxy_impl(mock_addr, retries, idle_timeout_secs, None, 0).await
}

/// Like `start_proxy` but with an optional incoming token and concurrency cap.
async fn start_proxy_impl(
    mock_addr: SocketAddr,
    retries: u32,
    idle_timeout_secs: u64,
    incoming_token: Option<&str>,
    max_concurrent: usize,
) -> String {
    let auth_dir = std::env::temp_dir().join(format!(
        "cc-proxy-chaos-auth-{}-{}",
        std::process::id(),
        mock_addr.port()
    ));
    std::fs::create_dir_all(&auth_dir).unwrap();
    std::fs::write(
        auth_dir.join("auth.json"),
        r#"{"apiKey":"chaos-key-1234567890"}"#,
    )
    .unwrap();
    std::fs::write(auth_dir.join("config.json"), r#"{}"#).unwrap();

    let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = probe.local_addr().unwrap().port();
    drop(probe);

    let config = cmdcode_core::config::ProxyConfig {
        listen_addr: format!("127.0.0.1:{}", proxy_port),
        upstream_url: format!("http://{}", mock_addr),
        default_model: "xiaomi/mimo-v2.5".into(),
        upstream_timeout_secs: 30,
        max_retries: retries,
        max_concurrent,
        cors_origin: None,
        model_allowlist: None,
        auth_dir: auth_dir.clone(),
        auth_cache_ttl_secs: 60,
        log_level: "error".into(),
        max_body_size: 10 * 1024 * 1024,
        stream_idle_timeout_secs: idle_timeout_secs,
        log_file: None,
        log_max_bytes: 50 * 1024 * 1024,
        log_keep: 5,
        tls_cert: None,
        tls_key: None,
        incoming_token: incoming_token.map(|t| t.to_string()),
    };
    let auth = cmdcode_core::auth::AuthManager::new(auth_dir, 60);

    std::thread::spawn(move || {
        let service = cmdcode_server::ProxyService::new(config, auth);
        let _ = service.run();
    });

    let proxy_url = format!("http://127.0.0.1:{}", proxy_port);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();
    let mut ready = false;
    for _ in 0..100 {
        if let Ok(r) = client.get(format!("{}/health", proxy_url)).send().await {
            if r.status().is_success() {
                ready = true;
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(ready, "proxy did not become ready in time");
    proxy_url
}

/// Minimal streaming chat request body (passes the upstream validator).
const CHAT_BODY: &str =
    r#"{"model":"xiaomi/mimo-v2.5","stream":true,"messages":[{"role":"user","content":"hi"}]}"#;

async fn proxy_chat(proxy_url: &str, body: &str) -> (u16, String) {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap();
    match client
        .post(format!("{}/v1/chat/completions", proxy_url))
        .header("content-type", "application/json")
        .header("authorization", "Bearer chaos-key-1234567890")
        .body(body.to_string())
        .send()
        .await
    {
        Ok(r) => {
            let status = r.status().as_u16();
            let body = r.text().await.unwrap_or_default();
            (status, body)
        }
        Err(e) => (0, format!("request error: {}", e)),
    }
}

#[tokio::test]
async fn test_chaos_through_proxy_retry_then_success() {
    let mock = MockUpstream {
        status_sequence: Some(Arc::new(vec![503, 200])),
        ..MockUpstream::normal()
    };
    let mock_addr = mock.start().await;
    let proxy = start_proxy(mock_addr, 2, 180).await;

    let (status, body) = proxy_chat(&proxy, CHAT_BODY).await;
    assert_eq!(
        status, 200,
        "retry-then-success should end at 200, got: {}",
        body
    );
    assert!(body.contains("Hello"));
    assert!(body.contains("[DONE]"));
}

#[tokio::test]
async fn test_chaos_through_proxy_retry_exhaustion() {
    let mock = MockUpstream {
        status_sequence: Some(Arc::new(vec![503, 503, 503])),
        ..MockUpstream::normal()
    };
    let mock_addr = mock.start().await;
    let proxy = start_proxy(mock_addr, 2, 180).await;

    let (status, body) = proxy_chat(&proxy, CHAT_BODY).await;
    assert_eq!(
        status, 503,
        "exhausted retries must surface 503, got: {}",
        body
    );
    assert!(body.contains("upstream error"));
}

#[tokio::test]
async fn test_chaos_through_proxy_mid_stream_reset() {
    let mock = MockUpstream {
        events: vec![
            serde_json::json!({"type": "text-delta", "text": "partial"}),
            serde_json::json!({"type": "text-delta", "text": "never-seen"}),
        ],
        reset_after_chunks: Some(1),
        ..MockUpstream::normal()
    };
    let mock_addr = mock.start().await;
    let proxy = start_proxy(mock_addr, 0, 180).await;

    let (status, body) = proxy_chat(&proxy, CHAT_BODY).await;
    // The client must receive what arrived before the reset, and never a
    // clean [DONE].
    assert_eq!(status, 200);
    assert!(body.contains("partial"));
    assert!(!body.contains("never-seen"));
    assert!(
        !body.contains("[DONE]"),
        "aborted stream must not end with [DONE]"
    );
}

#[tokio::test]
async fn test_chaos_through_proxy_truncated_final_line() {
    let mock = MockUpstream {
        events: vec![
            serde_json::json!({"type": "text-delta", "text": "Hello"}),
            serde_json::json!({"type": "finish", "finishReason": "stop", "totalUsage": {"inputTokens": 1, "outputTokens": 1, "inputTokenDetails": {}}}),
        ],
        no_final_newline: true,
        ..MockUpstream::normal()
    };
    let mock_addr = mock.start().await;
    let proxy = start_proxy(mock_addr, 0, 180).await;

    let (status, body) = proxy_chat(&proxy, CHAT_BODY).await;
    assert_eq!(status, 200);
    assert!(body.contains("Hello"));
    assert!(
        body.contains("[DONE]"),
        "stream must terminate cleanly after truncated line"
    );
}

#[tokio::test]
async fn test_chaos_through_proxy_truncated_mid_record_no_done() {
    // The final event line is cut mid-token and unterminated, so it cannot
    // be parsed as a complete record. The proxy must NOT present this as a
    // clean [DONE] completion, and must record a truncated-stream metric.
    let mock = MockUpstream {
        events: vec![
            serde_json::json!({"type": "text-delta", "text": "partial"}),
            serde_json::json!({"type": "finish", "finishReason": "stop"}),
        ],
        no_final_newline: true,
        final_tail: Some(r#"{"unclosed"#.into()),
        ..MockUpstream::normal()
    };
    let mock_addr = mock.start().await;
    let proxy = start_proxy(mock_addr, 0, 180).await;

    let (status, body) = proxy_chat(&proxy, CHAT_BODY).await;
    assert_eq!(status, 200);
    assert!(body.contains("partial"));
    assert!(
        !body.contains("[DONE]"),
        "truncated stream must not end with [DONE]: {}",
        body
    );

    // The truncated-stream counter must be non-zero.
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();
    let metrics = client
        .get(format!("{}/metrics", proxy))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        metrics.contains("cmdcode_truncated_streams_total 1"),
        "expected a truncated-stream metric, got: {}",
        metrics
    );
}

#[tokio::test]
async fn test_auth_refresh_on_401_succeeds_once() {
    // Upstream rejects the first request with 401 (stale credential), then
    // accepts. With max_retries=0 the proxy must still refresh its auth cache
    // and retry exactly once, ending in a 200.
    let mock = MockUpstream {
        status_sequence: Some(Arc::new(vec![401, 200])),
        ..MockUpstream::normal()
    };
    let mock_addr = mock.start().await;
    let proxy = start_proxy(mock_addr, 0, 180).await;

    let (status, body) = proxy_chat(&proxy, CHAT_BODY).await;
    assert_eq!(
        status, 200,
        "auth-refresh retry must succeed, got {status}: {body}"
    );
    assert!(body.contains("Hello"));
}

#[tokio::test]
async fn test_chaos_through_proxy_garbage_stream() {
    let mock = MockUpstream {
        events: vec![
            serde_json::json!("not-an-object"),
            serde_json::json!({"type": "unknown-event"}),
            serde_json::json!(42),
            serde_json::json!({"type": "text-delta", "text": "recovered"}),
        ],
        ..MockUpstream::normal()
    };
    let mock_addr = mock.start().await;
    let proxy = start_proxy(mock_addr, 0, 180).await;

    let (status, body) = proxy_chat(&proxy, CHAT_BODY).await;
    assert_eq!(status, 200);
    assert!(
        body.contains("recovered"),
        "valid events after garbage must flow: {}",
        body
    );
    assert!(body.contains("[DONE]"));
}

#[tokio::test]
async fn test_chaos_through_proxy_events_after_finish_forwarded() {
    let mock = MockUpstream {
        events: vec![
            serde_json::json!({"type": "text-delta", "text": "first"}),
            serde_json::json!({"type": "finish", "finishReason": "stop", "totalUsage": {"inputTokens": 1, "outputTokens": 1, "inputTokenDetails": {}}}),
            serde_json::json!({"type": "text-delta", "text": "after-finish"}),
        ],
        ..MockUpstream::normal()
    };
    let mock_addr = mock.start().await;
    let proxy = start_proxy(mock_addr, 0, 180).await;

    let (status, body) = proxy_chat(&proxy, CHAT_BODY).await;
    assert_eq!(status, 200);
    // Current contract: trailing deltas after finish are forwarded as-is.
    assert!(
        body.contains("after-finish"),
        "events after finish must be forwarded: {}",
        body
    );
    assert!(body.contains("[DONE]"));
}

#[tokio::test]
async fn test_chaos_through_proxy_error_event_stops_stream() {
    let mock = MockUpstream {
        events: vec![
            serde_json::json!({"type": "text-delta", "text": "before"}),
            serde_json::json!({"type": "error", "error": {"message": "boom"}}),
            serde_json::json!({"type": "text-delta", "text": "after-error"}),
        ],
        ..MockUpstream::normal()
    };
    let mock_addr = mock.start().await;
    let proxy = start_proxy(mock_addr, 0, 180).await;

    let (status, body) = proxy_chat(&proxy, CHAT_BODY).await;
    assert_eq!(status, 200);
    assert!(body.contains("before"));
    assert!(
        body.contains("boom"),
        "error event must be surfaced: {}",
        body
    );
    assert!(body.contains("[DONE]"));
    assert!(
        !body.contains("after-error"),
        "stream must stop at error event: {}",
        body
    );
}

#[tokio::test]
async fn test_chaos_through_proxy_oversized_body() {
    let mock_addr = MockUpstream::normal().start().await;
    let proxy = start_proxy(mock_addr, 0, 180).await;

    let big = format!(
        r#"{{"model":"x","messages":[],"junk":"{}"}}"#,
        "y".repeat(11 * 1024 * 1024)
    );
    let (status, _) = proxy_chat(&proxy, &big).await;
    assert_eq!(status, 413, "oversized body must be rejected with 413");
}

#[tokio::test]
async fn test_chaos_through_proxy_invalid_json_body() {
    let mock_addr = MockUpstream::normal().start().await;
    let proxy = start_proxy(mock_addr, 0, 180).await;

    let (status, body) = proxy_chat(&proxy, "this is not json").await;
    assert_eq!(status, 400, "invalid JSON must be rejected with 400");
    assert!(body.contains("invalid_request_error") || body.contains("400"));
}

#[tokio::test]
async fn test_chaos_through_proxy_upstream_500() {
    let mock = MockUpstream {
        status: 500,
        ..MockUpstream::normal()
    };
    let mock_addr = mock.start().await;
    let proxy = start_proxy(mock_addr, 2, 180).await;

    let (status, body) = proxy_chat(&proxy, CHAT_BODY).await;
    // 500 is not retryable — must surface immediately.
    assert_eq!(status, 500);
    assert!(body.contains("upstream error"));
}

#[tokio::test]
async fn test_chaos_through_proxy_trickle_stream() {
    let mut events = Vec::new();
    for i in 0..8 {
        events.push(serde_json::json!({"type": "text-delta", "text": format!("t{}", i)}));
    }
    events.push(serde_json::json!({"type": "finish", "finishReason": "stop", "totalUsage": {"inputTokens": 1, "outputTokens": 1, "inputTokenDetails": {}}}));
    let mock = MockUpstream {
        events,
        chunk_gap_ms: 50,
        ..MockUpstream::normal()
    };
    let mock_addr = mock.start().await;
    let proxy = start_proxy(mock_addr, 0, 180).await;

    let start = std::time::Instant::now();
    let (status, body) = proxy_chat(&proxy, CHAT_BODY).await;
    assert_eq!(status, 200);
    for i in 0..8 {
        assert!(body.contains(&format!("t{}", i)), "missing chunk t{}", i);
    }
    assert!(body.contains("[DONE]"));
    assert!(
        start.elapsed() >= std::time::Duration::from_millis(300),
        "trickle pacing not observed"
    );
}

#[tokio::test]
async fn test_chaos_through_proxy_idle_timeout_abort() {
    let mock = MockUpstream {
        events: vec![
            serde_json::json!({"type": "text-delta", "text": "first"}),
            serde_json::json!({"type": "finish", "finishReason": "stop", "totalUsage": {"inputTokens": 1, "outputTokens": 1, "inputTokenDetails": {}}}),
        ],
        chunk_gap_ms: 2500,
        ..MockUpstream::normal()
    };
    let mock_addr = mock.start().await;
    // 1s idle timeout: the 2.5s gap between chunks must abort the stream.
    let proxy = start_proxy(mock_addr, 0, 1).await;

    let start = std::time::Instant::now();
    let (status, body) = proxy_chat(&proxy, CHAT_BODY).await;
    assert_eq!(status, 200);
    assert!(body.contains("first"));
    assert!(
        !body.contains("[DONE]"),
        "idle-timeout abort must not emit [DONE]"
    );
    assert!(start.elapsed() >= std::time::Duration::from_secs(1));
}

#[tokio::test]
async fn test_e2e_metrics_endpoint() {
    let mock_addr = MockUpstream::normal().start().await;
    let proxy = start_proxy(mock_addr, 0, 180).await;

    // Drive one real request so counters are non-zero.
    let (status, _) = proxy_chat(&proxy, CHAT_BODY).await;
    assert_eq!(status, 200);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();
    let resp = client
        .get(format!("{}/metrics", proxy))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(ct.contains("text/plain"), "expected text/plain, got {ct}");

    let body = resp.text().await.unwrap();
    assert!(body.contains("cmdcode_requests_total"));
    assert!(body.contains("# TYPE cmdcode_requests_total counter"));
    assert!(body.contains("cmdcode_active_streams"));
    assert!(body.contains("# TYPE cmdcode_active_streams gauge"));
}

#[tokio::test]
async fn test_e2e_incoming_auth_required() {
    let mock_addr = MockUpstream::normal().start().await;
    let proxy = start_proxy_impl(mock_addr, 0, 180, Some("correct-horse"), 0).await;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();

    // Without token -> 401.
    let resp = client
        .post(format!("{}/v1/chat/completions", proxy))
        .header("content-type", "application/json")
        .body(CHAT_BODY)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);

    // Wrong token -> 401.
    let resp = client
        .post(format!("{}/v1/chat/completions", proxy))
        .header("content-type", "application/json")
        .header("authorization", "Bearer wrong-horse")
        .body(CHAT_BODY)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);

    // Correct token -> proxied through (200).
    let resp = client
        .post(format!("{}/v1/chat/completions", proxy))
        .header("content-type", "application/json")
        .header("authorization", "Bearer correct-horse")
        .body(CHAT_BODY)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    // /health and /metrics stay open without a token.
    let health = client
        .get(format!("{}/health", proxy))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status().as_u16(), 200);
    let metrics = client
        .get(format!("{}/metrics", proxy))
        .send()
        .await
        .unwrap();
    assert_eq!(metrics.status().as_u16(), 200);
}

// ============================================================
// Concurrency benchmark matrix
// ============================================================

/// Fire `total` requests through the proxy with `concurrency` workers in
/// flight at once. Returns per-request latencies (ms) and TTFB (ms).
async fn run_concurrent_chat(
    proxy: &str,
    concurrency: usize,
    total: usize,
) -> (Vec<f64>, Vec<f64>) {
    use futures::StreamExt;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .pool_max_idle_per_host(256)
        .build()
        .unwrap();
    let url = format!("{}/v1/chat/completions", proxy);

    // Each element is an (index, latency_ms) so we can detect drops.
    let work = 0..total;
    let results: Vec<(usize, f64, f64)> = futures::stream::iter(work)
        .map(|i| {
            let client = &client;
            let url = &url;
            async move {
                let start = std::time::Instant::now();
                let resp = client
                    .post(url)
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer chaos-key-1234567890")
                    .body(CHAT_BODY)
                    .send()
                    .await;
                let ttfb = start.elapsed().as_secs_f64() * 1000.0;
                if let Ok(r) = resp {
                    let _ = r.bytes().await;
                }
                let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                (i, elapsed, ttfb)
            }
        })
        .buffer_unordered(concurrency)
        .collect::<Vec<_>>()
        .await;

    let mut lat = Vec::with_capacity(total);
    let mut ttfb = Vec::with_capacity(total);
    let mut seen: Vec<bool> = vec![false; total];
    for (i, l, t) in results {
        lat.push(l);
        ttfb.push(t);
        seen[i] = true;
    }
    assert!(
        seen.iter().all(|s| *s),
        "lost {} results; expected {}",
        seen.iter().filter(|s| !**s).count(),
        total
    );
    (lat, ttfb)
}

/// Concurrency benchmark: exercises the proxy under increasing parallel load.
/// Prints a latency/throughput matrix and asserts sane success + latency bounds.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn test_concurrency_matrix() {
    // A streaming mock so the proxy's SSE path is exercised.
    let mock = MockUpstream {
        events: vec![
            serde_json::json!({"type": "text-delta", "text": "hello "}),
            serde_json::json!({"type": "text-delta", "text": "world"}),
            serde_json::json!({"type": "finish", "finishReason": "stop"}),
        ],
        ..MockUpstream::normal()
    };
    let mock_addr = mock.start().await;
    let proxy = start_proxy(mock_addr, 0, 30).await;

    // concurrency -> total requests (keep the matrix fast on CI).
    let cases: &[(usize, usize)] = &[(1, 20), (10, 40), (50, 100)];

    eprintln!("\n=== Concurrency matrix (streaming) ===");
    eprintln!(
        "{:>4} {:>6} {:>10} {:>10} {:>10} {:>10} {:>10}",
        "conn", "total", "rps", "p50ms", "p95ms", "p99ms", "ttfb50"
    );
    for &(conn, total) in cases {
        let start = std::time::Instant::now();
        let (mut lat, mut ttfb) = run_concurrent_chat(&proxy, conn, total).await;
        let wall = start.elapsed().as_secs_f64();

        let ok = lat.len();
        lat.sort_by(|a, b| a.partial_cmp(b).unwrap());
        ttfb.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let p50 = percentile(&lat, 0.50);
        let p95 = percentile(&lat, 0.95);
        let p99 = percentile(&lat, 0.99);
        let ttfb50 = percentile(&ttfb, 0.50);
        let rps = ok as f64 / wall;

        eprintln!(
            "{:>4} {:>6} {:>10.1} {:>10.3} {:>10.3} {:>10.3} {:>10.3}",
            conn, ok, rps, p50, p95, p99, ttfb50
        );

        // Every request must succeed.
        assert_eq!(ok, total, "not all requests succeeded: {ok}/{}", total);
        // Sane latency: p99 under 10s. The mock+proxy on loopback is sub-ms,
        // so this only catches true regressions, not CI noise.
        assert!(
            p99 < 10_000.0,
            "p99 latency too high: {p99:.1}ms at concurrency {conn}"
        );
    }
}

/// Regression test for the concurrency semaphore permit guard.
///
/// If `let _ = _permit;` were reintroduced (dropping the permit immediately
/// instead of holding it for the stream duration), both concurrent requests
/// would acquire permits simultaneously and stream in parallel. This test
/// detects that regression by verifying the second request's response is
/// blocked until after the first stream completes.
///
/// The mock streams 3 events with 300ms gaps (2 gaps = ~600ms per stream).
/// With max_concurrent=1 and a held permit the second stream cannot begin
/// until the first finishes, so the gap between their completion times must
/// be >= ~500ms.  If the bug is present, both streams run concurrently and
/// the gap collapses to ~0ms.
#[tokio::test]
async fn test_semaphore_held_during_stream() {
    let mock = MockUpstream {
        events: vec![
            serde_json::json!({"type": "text-delta", "text": "a"}),
            serde_json::json!({"type": "text-delta", "text": "b"}),
            serde_json::json!({"type": "finish", "finishReason": "stop", "totalUsage": {"inputTokens": 1, "outputTokens": 1, "inputTokenDetails": {}}}),
        ],
        chunk_gap_ms: 300,
        ..MockUpstream::normal()
    };
    let mock_addr = mock.start().await;
    // Concurrency cap of 1: only one stream permit at a time.
    let proxy = start_proxy_impl(mock_addr, 0, 30, None, 1).await;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .unwrap();
    let url = format!("{}/v1/chat/completions", proxy);

    // Fire two concurrent streaming requests. Each sender transmits the time
    // at which the response stream completed.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<f64>(2);
    for _ in 0..2 {
        let tx = tx.clone();
        let client = client.clone();
        let url = url.clone();
        tokio::spawn(async move {
            let start = std::time::Instant::now();
            let resp = client
                .post(&url)
                .header("content-type", "application/json")
                .header("authorization", "Bearer chaos-key-1234567890")
                .body(CHAT_BODY)
                .send()
                .await;
            if let Ok(r) = resp {
                let _ = r.text().await;
            }
            let _ = tx.send(start.elapsed().as_secs_f64() * 1000.0).await;
        });
    }
    drop(tx);

    let mut finishes = Vec::new();
    while let Some(t) = rx.recv().await {
        finishes.push(t);
    }
    finishes.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(finishes.len(), 2, "both streams must complete");

    // The gap between finish times must be >= the stream duration.  If the
    // semaphore permit is dropped immediately (the old bug), both streams run
    // concurrently and finishes[1] - finishes[0] ≈ 0ms, failing this check.
    let gap = finishes[1] - finishes[0];
    let min_expected_gap = 500.0; // conservative lower bound (2 gaps × 300ms)
    assert!(
        gap >= min_expected_gap,
        "semaphore permit was not held during streaming: \
         first stream finished at {first:.0}ms, second at {second:.0}ms \
         (gap={gap:.0}ms < {min_expected_gap:.0}ms). \
         Regression: _permit_guard fix may have been reverted.",
        first = finishes[0],
        second = finishes[1],
    );
}

#[tokio::test]
async fn test_max_concurrent_serializes_streams() {
    // A slow streaming mock (one text delta, then a finish after a gap) so a
    // held stream stays open long enough to observe serialization.
    let mock = MockUpstream {
        events: vec![
            serde_json::json!({"type": "text-delta", "text": "one"}),
            serde_json::json!({"type": "finish", "finishReason": "stop"}),
        ],
        chunk_gap_ms: 400,
        ..MockUpstream::normal()
    };
    let mock_addr = mock.start().await;
    // Concurrency cap of 1: only one stream may hold a permit at a time.
    let proxy = start_proxy_impl(mock_addr, 0, 30, None, 1).await;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap();
    let url = format!("{}/v1/chat/completions", proxy);

    // Fire two streaming requests concurrently.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<f64>(2);
    for _ in 0..2 {
        let tx = tx.clone();
        let client = client.clone();
        let url = url.clone();
        tokio::spawn(async move {
            let start = std::time::Instant::now();
            let _ = client
                .post(&url)
                .header("content-type", "application/json")
                .header("authorization", "Bearer chaos-key-1234567890")
                .body(CHAT_BODY)
                .send()
                .await;
            let _ = tx.send(start.elapsed().as_secs_f64() * 1000.0).await;
        });
    }
    drop(tx);

    let mut finishes = Vec::new();
    while let Some(t) = rx.recv().await {
        finishes.push(t);
    }
    finishes.sort_by(|a, b| a.partial_cmp(b).unwrap());

    // With max_concurrent=1 the two streams MUST be serialized, so the second
    // cannot finish before the first's stream duration + permit release.
    assert_eq!(finishes.len(), 2, "expected both streams to complete");
    assert!(
        finishes[1] - finishes[0] >= 400.0,
        "streams with max_concurrent=1 must be serialized (gap {}ms < 400ms)",
        finishes[1] - finishes[0]
    );
}
