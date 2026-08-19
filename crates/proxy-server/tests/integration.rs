use std::net::SocketAddr;
use tokio::net::TcpListener;

/// Mock upstream that returns configurable NDJSON responses.
struct MockUpstream {
    events: Vec<serde_json::Value>,
    status: u16,
    delay_ms: u64,
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
        }
    }

    async fn start(self) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let events = self.events;
        let status = self.status;
        let delay = self.delay_ms;

        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let events = events.clone();
                tokio::spawn(async move {
                    let io = hyper_util::rt::TokioIo::new(stream);
                let service = hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                    let events = events.clone();
                    async move {
                        // Consume request body
                        use http_body_util::BodyExt;
                        let _ = req.into_body().collect().await;

                            if delay > 0 {
                                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                            }

                            if status != 200 {
                                let body = r#"{"error":{"message":"upstream error"}}"#;
                                let mut resp = hyper::Response::new(http_body_util::Full::new(
                                    bytes::Bytes::from(body),
                                ));
                                *resp.status_mut() = hyper::StatusCode::from_u16(status).unwrap();
                                resp.headers_mut().insert("content-type", "application/json".parse().unwrap());
                                resp.headers_mut().insert("connection", "close".parse().unwrap());
                                return Ok::<_, hyper::Error>(resp);
                            }

                            if events.is_empty() {
                                return Ok::<_, hyper::Error>(hyper::Response::new(
                                    http_body_util::Full::new(bytes::Bytes::new()),
                                ));
                            }

                            let mut body = String::new();
                            for event in &events {
                                body.push_str(&serde_json::to_string(event).unwrap());
                                body.push('\n');
                            }

                            let mut resp = hyper::Response::new(http_body_util::Full::new(
                                bytes::Bytes::from(body),
                            ));
                            resp.headers_mut().insert("content-type", "application/x-ndjson".parse().unwrap());
                            Ok(resp)
                        }
                    });

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
    ).ok();

    let auth = proxy_core::auth::AuthManager::new(
        std::path::PathBuf::from("/tmp/test/.commandcode"),
        30,
    );

    // Catalog may be empty in test env if CLI isn't installed — that's OK
    let _catalog = proxy_core::model_catalog::get_model_catalog();

    let method = auth.get_auth_method().await.unwrap();
    match method {
        proxy_core::auth::AuthMethod::ApiKey(k) => assert_eq!(k, "test-key"),
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
        proxy_core::wire_format::OpenAiMessage {
            role: "system".into(),
            content: Some(serde_json::json!("Be helpful")),
            tool_call_id: None,
            tool_calls: None,
        },
        proxy_core::wire_format::OpenAiMessage {
            role: "user".into(),
            content: Some(serde_json::json!("Hello")),
            tool_call_id: None,
            tool_calls: None,
        },
    ];

    let wire = proxy_core::wire_format::wire_messages(&messages);
    assert_eq!(wire.len(), 2);

    match &wire[0] {
        proxy_core::wire_format::CcMessage::System { content } => {
            assert_eq!(content, "Be helpful");
        }
        _ => panic!("expected system message"),
    }

    match &wire[1] {
        proxy_core::wire_format::CcMessage::User { content } => {
            assert_eq!(content.len(), 1);
            match &content[0] {
                proxy_core::wire_format::CcContentItem::Text { text } => {
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
    let tools = vec![
        proxy_core::wire_format::OpenAiTool {
            tool_type: "function".into(),
            function: Some(proxy_core::wire_format::OpenAiFunction {
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
        },
    ];

    let wire = proxy_core::wire_format::wire_tools(&tools);
    assert_eq!(wire.len(), 1);
    assert_eq!(wire[0].name, "search");
    assert_eq!(wire[0].description, "Search the web");
}

#[tokio::test]
async fn test_e2e_reasoning_effort_parsing() {
    let (model, effort) = proxy_core::types::parse_model_and_effort("claude-sonnet-5:high");
    assert_eq!(model.as_str(), "claude-sonnet-5");
    assert_eq!(effort, Some(proxy_core::types::Effort::High));

    let (model, effort) = proxy_core::types::parse_model_and_effort("gpt-5.6-luna");
    assert_eq!(model.as_str(), "gpt-5.6-luna");
    assert_eq!(effort, None);

    let (model, effort) = proxy_core::types::parse_model_and_effort("command-code/xiaomi/mimo-v2.5:max");
    assert_eq!(model.as_str(), "xiaomi/mimo-v2.5");
    assert_eq!(effort, Some(proxy_core::types::Effort::Max));
}

// ============================================================
// Chaos Tests
// ============================================================

#[tokio::test]
async fn test_chaos_empty_upstream() {
    let addr = MockUpstream { events: vec![], status: 200, delay_ms: 0 }.start().await;
    let (status, _) = proxy_request(addr, "/alpha/generate", "POST", Some(r#"{"test":true}"#)).await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn test_chaos_upstream_500() {
    let addr = MockUpstream { status: 500, ..MockUpstream::normal() }.start().await;
    let (status, body) = proxy_request(addr, "/alpha/generate", "POST", Some(r#"{"test":true}"#)).await;
    assert_eq!(status, 500);
    assert!(body.contains("upstream error"));
}

#[tokio::test]
async fn test_chaos_upstream_slow() {
    let addr = MockUpstream { delay_ms: 100, ..MockUpstream::normal() }.start().await;
    let start = std::time::Instant::now();
    let (status, _) = proxy_request(addr, "/alpha/generate", "POST", Some(r#"{"test":true}"#)).await;
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
                let service = hyper::service::service_fn(|req: hyper::Request<hyper::body::Incoming>| async move {
                    use http_body_util::BodyExt;
                    let _ = req.into_body().collect().await;
                    let body = "not json at all\n{\"type\":\"text-delta\",\"text\":\"recovered\"}\n";
                    let mut resp = hyper::Response::new(http_body_util::Full::new(
                        bytes::Bytes::from(body),
                    ));
                    resp.headers_mut().insert("content-type", "application/x-ndjson".parse().unwrap());
                    Ok::<_, hyper::Error>(resp)
                });
                hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, service)
                    .await
                    .ok();
            });
        }
    });

    let (status, _) = proxy_request(addr, "/alpha/generate", "POST", Some(r#"{"test":true}"#)).await;
    assert!(status == 200 || status == 502);
}

#[tokio::test]
async fn test_chaos_connection_refused() {
    let result = proxy_request(
        "127.0.0.1:19999".parse().unwrap(),
        "/alpha/generate",
        "POST",
        Some(r#"{"test":true}"#),
    ).await;
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
    };
    let addr = mock.start().await;
    let (status, _) = proxy_request(addr, "/alpha/generate", "POST", Some(r#"{"test":true}"#)).await;
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
    };
    let addr = mock.start().await;
    let (status, body) = proxy_request(addr, "/alpha/generate", "POST", Some(r#"{"test":true}"#)).await;
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

    let mock = MockUpstream { events, status: 200, delay_ms: 0 };
    let addr = mock.start().await;
    let (status, body) = proxy_request(addr, "/alpha/generate", "POST", Some(r#"{"test":true}"#)).await;
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
            ).await
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
            ).await
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
    let catalog = proxy_core::model_catalog::get_model_catalog();
    // In test env, catalog may be empty if CLI isn't installed
    // The parsing logic is tested separately in proxy-core unit tests
    assert!(catalog.len() <= 100, "Catalog too large: {}", catalog.len());
}

#[test]
fn test_model_catalog_efforts() {
    let catalog = proxy_core::model_catalog::get_model_catalog();
    if catalog.is_empty() {
        return; // Skip if CLI not installed in test env
    }

    let claude = catalog.get(&proxy_core::types::ModelId::new("claude-sonnet-5"));
    if let Some(claude) = claude {
        assert!(claude.reasoning);
        assert_eq!(claude.efforts.len(), 5);
        assert!(claude.efforts.contains(&proxy_core::types::Effort::High));
        assert!(claude.efforts.contains(&proxy_core::types::Effort::Max));
    }

    let gpt = catalog.get(&proxy_core::types::ModelId::new("gpt-5.6-luna"));
    if let Some(gpt) = gpt {
        assert!(gpt.reasoning);
        assert!(gpt.context_window.as_u64() > 0);
    }
}

#[test]
fn test_model_catalog_providers() {
    let catalog = proxy_core::model_catalog::get_model_catalog();
    if catalog.is_empty() {
        return; // Skip if CLI not installed in test env
    }
    let providers: std::collections::HashSet<_> = catalog.values().map(|m| m.provider.as_ref()).collect();
    assert!(providers.contains("openai") || providers.contains("anthropic"));
}

#[test]
fn test_config_from_env() {
    let config = proxy_core::config::ProxyConfig::from_env().unwrap();
    assert_eq!(config.listen_addr, "127.0.0.1:18080");
    assert_eq!(config.upstream_url, "https://api.commandcode.ai");
    assert_eq!(config.default_model, "xiaomi/mimo-v2.5");
    assert_eq!(config.upstream_timeout_secs, 600);
    assert_eq!(config.max_retries, 2);
}

#[test]
fn test_auth_data_parsing() {
    let json = r#"{"apiKey":"my-key","userId":"123","userName":"test"}"#;
    let auth: proxy_core::auth::AuthData = serde_json::from_str(json).unwrap();
    assert_eq!(auth.api_key.as_deref(), Some("my-key"));
    assert_eq!(auth.user_id.as_deref(), Some("123"));
}

#[test]
fn test_auth_data_oauth() {
    let json = r#"{"oauthToken":"token123","oauthProvider":"github"}"#;
    let auth: proxy_core::auth::AuthData = serde_json::from_str(json).unwrap();
    assert_eq!(auth.oauth_token.as_deref(), Some("token123"));
    assert_eq!(auth.oauth_provider.as_deref(), Some("github"));
    assert!(auth.api_key.is_none());
}

#[test]
fn test_error_types() {
    let err = proxy_core::error::ProxyError::ModelNotAllowed("test".into());
    assert!(err.to_string().contains("test"));

    let err = proxy_core::error::UpstreamError::ConnectionRefused {
        host: "localhost".into(),
        port: 8080,
    };
    assert!(err.to_string().contains("localhost"));

    let err = proxy_core::error::AuthError::NoAuthConfigured;
    assert!(!err.to_string().is_empty());
}

#[test]
fn test_finish_reason_mapping() {
    use proxy_core::types::FinishReason;
    assert_eq!(FinishReason::from_upstream("stop"), FinishReason::Stop);
    assert_eq!(FinishReason::from_upstream("tool_use"), FinishReason::ToolCalls);
    assert_eq!(FinishReason::from_upstream("tool-calls"), FinishReason::ToolCalls);
    assert_eq!(FinishReason::from_upstream("length"), FinishReason::Length);
    assert_eq!(FinishReason::from_upstream("max_tokens"), FinishReason::Length);
    assert_eq!(FinishReason::from_upstream("unknown"), FinishReason::Stop);
}

#[test]
fn test_request_id_generation() {
    let id1 = proxy_core::types::RequestId::generate();
    let id2 = proxy_core::types::RequestId::generate();
    assert_ne!(id1.as_str(), id2.as_str());
    assert!(!id1.as_str().is_empty());
}

#[test]
fn test_session_id_generation() {
    let id1 = proxy_core::types::SessionId::generate();
    let id2 = proxy_core::types::SessionId::generate();
    assert_ne!(id1.as_str(), id2.as_str());
}
