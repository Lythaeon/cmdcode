use cmdcode_core::auth::AuthManager;
use cmdcode_core::config::ProxyConfig;
use std::path::PathBuf;
use std::time::Duration;

pub fn run() {
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("FAIL: failed to create tokio runtime: {e}");
            std::process::exit(1);
        }
    };
    rt.block_on(run_inner());
}

async fn run_inner() {
    println!("cmdcode test\n");

    // Step 1: Check auth
    println!("[1/4] Checking authentication...");
    let auth_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".commandcode");
    let auth_file = auth_dir.join("auth.json");

    if !auth_file.exists() {
        eprintln!("  FAIL: auth.json not found at {}", auth_file.display());
        eprintln!();
        eprintln!("Install and log in:");
        eprintln!("  npm install -g command-code");
        eprintln!("  command-code login");
        std::process::exit(1);
    }

    let auth_content = match std::fs::read_to_string(&auth_file) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  FAIL: cannot read auth.json: {e}");
            std::process::exit(1);
        }
    };

    let auth: serde_json::Value = match serde_json::from_str(&auth_content) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("  FAIL: invalid auth.json: {e}");
            std::process::exit(1);
        }
    };

    let has_api_key = auth
        .get("apiKey")
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    let has_oauth = auth
        .get("oauthToken")
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);

    if !has_api_key && !has_oauth {
        eprintln!("  FAIL: no credentials (need apiKey or oauthToken)");
        eprintln!("  Run: command-code login");
        std::process::exit(1);
    }

    println!("  OK (credentials found)");

    // Step 2: Check config
    println!("[2/4] Checking configuration...");
    let config = match ProxyConfig::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  FAIL: invalid config: {e}");
            std::process::exit(1);
        }
    };
    println!("  OK (upstream: {})", config.upstream_url);

    // Step 3: Start proxy, send test request, verify response
    println!("[3/4] Starting proxy and sending test request...");

    // Pick a free port
    let probe = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("FAIL: failed to bind probe port: {e}");
            std::process::exit(1);
        }
    };
    let proxy_port = match probe.local_addr() {
        Ok(addr) => addr.port(),
        Err(e) => {
            eprintln!("FAIL: probe port address unavailable: {e}");
            std::process::exit(1);
        }
    };
    drop(probe);

    let test_config = ProxyConfig {
        listen_addr: format!("127.0.0.1:{}", proxy_port),
        ..config
    };

    let auth_manager = AuthManager::new(
        test_config.auth_dir.clone(),
        test_config.auth_cache_ttl_secs,
    );

    // Start proxy in background thread
    std::thread::spawn(move || {
        let service = cmdcode_server::ProxyService::new(test_config, auth_manager);
        let _ = service.run();
    });

    let proxy_url = format!("http://127.0.0.1:{}", proxy_port);
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("FAIL: failed to build HTTP client: {e}");
            std::process::exit(1);
        }
    };

    // Wait for proxy to be ready
    let mut ready = false;
    for _ in 0..50 {
        if let Ok(r) = client.get(format!("{}/health", proxy_url)).send().await {
            if r.status().is_success() {
                ready = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    if !ready {
        eprintln!("  FAIL: proxy did not start within 10 seconds");
        std::process::exit(1);
    }

    // Send test request
    let body = serde_json::json!({
        "model": "xiaomi/mimo-v2.5",
        "messages": [{"role": "user", "content": "Say exactly: test successful"}],
        "max_tokens": 50,
    });

    let start = std::time::Instant::now();
    let resp = match client
        .post(format!("{}/v1/chat/completions", proxy_url))
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", get_test_token(&auth)))
        .body(serde_json::to_string(&body).unwrap_or_default())
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("  FAIL: request failed: {e}");
            std::process::exit(1);
        }
    };

    let status = resp.status().as_u16();
    let response_body = resp.text().await.unwrap_or_default();
    let elapsed = start.elapsed();

    if status != 200 {
        eprintln!("  FAIL: upstream returned HTTP {status}");
        eprintln!("  Response: {response_body}");
        std::process::exit(1);
    }

    // Parse response
    let response: serde_json::Value = match serde_json::from_str(&response_body) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("  FAIL: invalid JSON response: {e}");
            eprintln!("  Raw: {response_body}");
            std::process::exit(1);
        }
    };

    // Validate response structure
    if response.get("id").is_none() {
        eprintln!("  FAIL: response missing 'id' field");
        std::process::exit(1);
    }

    let choices = response.get("choices").and_then(|c| c.as_array());
    let choice = choices.and_then(|c| c.first());
    if choice.is_none() {
        eprintln!("  FAIL: response missing or empty 'choices' array");
        std::process::exit(1);
    }

    let content = choice
        .and_then(|m| m.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("");

    println!(
        "  OK (HTTP {status}, {}ms, content: {:?})",
        elapsed.as_millis(),
        if content.len() > 60 {
            format!("{}...", &content[..57])
        } else {
            content.to_string()
        }
    );

    // Step 4: Check models endpoint
    println!("[4/4] Checking /v1/models endpoint...");
    let models_resp = match client.get(format!("{}/v1/models", proxy_url)).send().await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("  FAIL: models request failed: {e}");
            std::process::exit(1);
        }
    };

    let models_status = models_resp.status().as_u16();
    let models_body = models_resp.text().await.unwrap_or_default();

    if models_status != 200 {
        eprintln!("  FAIL: /v1/models returned HTTP {models_status}");
        std::process::exit(1);
    }

    let models: serde_json::Value = match serde_json::from_str(&models_body) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("  FAIL: invalid JSON from /v1/models: {e}");
            std::process::exit(1);
        }
    };

    let model_count = models
        .get("data")
        .and_then(|d| d.as_array())
        .map(|a| a.len())
        .unwrap_or(0);

    println!("  OK ({} models available)", model_count);

    println!();
    println!("All checks passed. Proxy is functional.");

    // Give the background thread a moment to clean up
    tokio::time::sleep(Duration::from_millis(100)).await;
}

fn get_test_token(auth: &serde_json::Value) -> String {
    if let Some(key) = auth.get("apiKey").and_then(|v| v.as_str()) {
        if !key.is_empty() {
            return key.to_string();
        }
    }
    if let Some(token) = auth.get("oauthToken").and_then(|v| v.as_str()) {
        if !token.is_empty() {
            return token.to_string();
        }
    }
    "test-token".to_string()
}
