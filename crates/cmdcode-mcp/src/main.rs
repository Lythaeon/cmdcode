//! MCP server for Command Code taste learning.
//!
//! Exposes a `taste` tool that agents can call to record coding preferences.
//! The tool calls the upstream API (free — no credit cost) to analyze instructions,
//! then writes results to `~/.commandcode/taste/taste.md`.
//!
//! Usage: `cmdcode-mcp` (stdio transport for MCP clients)

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::or_fun_call)]

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn taste_dir() -> PathBuf {
    home().join(".commandcode").join("taste")
}

fn taste_file() -> PathBuf {
    taste_dir().join("taste.md")
}

fn upstream_url() -> String {
    std::env::var("COMMAND_CODE_API_BASE")
        .unwrap_or_else(|_| "https://api.commandcode.ai".into())
}

fn upstream_model() -> String {
    std::env::var("COMMAND_CODE_PROXY_DEFAULT")
        .unwrap_or_else(|_| "xiaomi/mimo-v2.5".into())
}

// --- MCP JSON-RPC server ---

fn read_jsonrpc() -> Option<Value> {
    let stdin = std::io::stdin();
    let mut line = String::new();
    BufReader::new(stdin).read_line(&mut line).ok()?;
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    serde_json::from_str(trimmed).ok()
}

fn write_jsonrpc(id: Option<Value>, result: Value) {
    let resp = json!({"jsonrpc": "2.0", "result": result, "id": id});
    let mut stdout = std::io::stdout();
    let _ = writeln!(stdout, "{}", serde_json::to_string(&resp).unwrap_or_default());
    let _ = stdout.flush();
}

fn write_jsonrpc_error(id: Option<Value>, code: i64, message: String) {
    let resp = json!({"jsonrpc": "2.0", "error": {"code": code, "message": message}, "id": id});
    let mut stdout = std::io::stdout();
    let _ = writeln!(stdout, "{}", serde_json::to_string(&resp).unwrap_or_default());
    let _ = stdout.flush();
}

// --- Taste tool ---

fn taste_tool_schema() -> Value {
    json!({
        "name": "taste",
        "description": "Record or update the user's coding preferences and taste. Use this whenever the user states a preference or asks you to remember something.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "instruction": {
                    "type": "string",
                    "description": "The preference or taste change to record, in the user's words."
                }
            },
            "required": ["instruction"]
        }
    })
}

async fn handle_taste_call(instruction: &str) -> Result<String, String> {
    if instruction.trim().is_empty() {
        return Err("An instruction is required.".into());
    }

    // Read current taste structure and content
    let taste_structure = get_taste_structure();
    let current_taste = read_taste_content();

    // Build the learning prompt (same as command-code agent)
    let user_msg = if current_taste.trim().is_empty() {
        format!(
            "No taste preferences recorded yet. This is the first taste entry.\n\
             NEW message to analyze:\n{}",
            serde_json::to_string(&serde_json::json!([{"role":"user","content":instruction}]))
                .unwrap_or_default()
        )
    } else {
        format!(
            "Current taste file content:\n---\n{current_taste}\n---\n\n\
             Taste structure: {taste_structure}\n\n\
             NEW message to analyze (learn ONLY from this):\n{}",
            serde_json::to_string(&serde_json::json!([{"role":"user","content":instruction}]))
                .unwrap_or_default()
        )
    };

    let body = json!({
        "config": {
            "workingDir": std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| ".".to_string()),
            "date": chrono_date(),
            "environment": "linux",
            "structure": [],
            "isGitRepo": false,
            "currentBranch": "",
            "mainBranch": "",
            "gitStatus": "",
            "recentCommits": []
        },
        "memory": null,
        "taste": null,
        "skills": null,
        "permissionMode": "standard",
        "mode": "agent",
        "params": {
            "model": upstream_model(),
            "messages": [{"role": "user", "content": user_msg}],
            "tools": [json!({
                "name": "write_taste_file",
                "description": "Write a taste category file. Use this to record preferences.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Path like 'style.md' or 'tools.md'"},
                        "content": {"type": "string", "description": "The content to write."}
                    },
                    "required": ["path", "content"]
                }
            })],
            "max_tokens": 2048,
            "stream": false
        }
    });

    // Call upstream
    let url = format!("{}/alpha/generate", upstream_url());
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    // Build auth headers from vault or legacy auth.json
    let headers = build_auth_headers().await;
    let resp = client
        .post(&url)
        .headers(headers)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("upstream request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body_text = resp.text().await.unwrap_or_default();
        return Err(format!("upstream error {status}: {body_text}"));
    }

    // Parse response - the model returns tool calls
    let response: Value = resp
        .json()
        .await
        .map_err(|e| format!("invalid upstream response: {e}"))?;

    // Extract content from the response
    let content = response
        .get("content")
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();

    let mut results = Vec::new();
    for item in &content {
        if item.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
            let tool_name = item.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let input = item.get("input").cloned().unwrap_or(json!({}));

            if tool_name == "write_taste_file" {
                if let (Some(path), Some(content)) = (
                    input.get("path").and_then(|p| p.as_str()),
                    input.get("content").and_then(|c| c.as_str()),
                ) {
                    let taste_path = resolve_taste_path(path);
                    if let Some(p) = taste_path {
                        std::fs::create_dir_all(p.parent().unwrap_or(Path::new(".")))
                            .map_err(|e| format!("mkdir failed: {e}"))?;
                        std::fs::write(&p, content)
                            .map_err(|e| format!("write failed: {e}"))?;
                        results.push(format!("Recorded preferences in {}", p.display()));
                    }
                }
            }
        }
    }

    if results.is_empty() {
        Ok("No new taste recorded.".into())
    } else {
        Ok(format!("Recorded: {}", results.join(", ")))
    }
}

// --- Helpers ---

fn read_taste_content() -> String {
    let global = taste_file();
    if global.exists() {
        std::fs::read_to_string(&global).unwrap_or_default()
    } else {
        String::new()
    }
}

fn get_taste_structure() -> String {
    let dir = taste_dir();
    if !dir.exists() {
        return "No preferences learned yet.".into();
    }
    let entries: Vec<String> = std::fs::read_dir(&dir)
        .ok()
        .and_then(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| !n.starts_with('.'))
                .collect::<Vec<_>>()
                .into()
        })
        .unwrap_or_default();

    if entries.is_empty() {
        "No preferences learned yet.".into()
    } else {
        entries.join(", ")
    }
}

fn resolve_taste_path(relative: &str) -> Option<PathBuf> {
    // Taste files live in ~/.commandcode/taste/ or project-local .commandcode/taste/
    let p = Path::new(relative);
    // Only allow taste.md or category/taste.md paths
    if p.file_name() == Some(std::ffi::OsStr::new("taste.md")) {
        let full = taste_dir().join(p);
        Some(full)
    } else {
        None
    }
}

fn chrono_date() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let days = now.as_secs() / 86400;
    let (y, m, d) = civil_from_days(days as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

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

async fn build_auth_headers() -> reqwest::header::HeaderMap {
    use reqwest::header;

    let mut headers = header::HeaderMap::new();
    headers.insert("Content-Type", "application/json".parse().unwrap());
    headers.insert("User-Agent", "cli".parse().unwrap());

    // Try vault first
    let vault = cmdcode_core::accounts::AccountStore::default();
    if let Ok(v) = vault.load() {
        if let Some(acct) = v.active_account() {
            let key = acct.api_key.as_str();
            if !key.is_empty() {
                headers.insert(
                    header::AUTHORIZATION,
                    format!("Bearer {key}").parse().unwrap(),
                );
            }
        }
    }

    // Fallback to legacy auth.json
    if !headers.contains_key("authorization") {
        let auth_file = home().join(".commandcode").join("auth.json");
        if let Ok(content) = std::fs::read_to_string(&auth_file) {
            if let Ok(auth) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(key) = auth.get("apiKey").and_then(|k| k.as_str()) {
                    headers.insert(
                        header::AUTHORIZATION,
                        format!("Bearer {key}").parse().unwrap(),
                    );
                }
            }
        }
    }

    headers.insert("x-cli-environment", "production".parse().unwrap());
    headers.insert("x-command-code-version", "1.0.0".parse().unwrap());
    headers.insert(
        "x-project-slug",
        "cmdcode-mcp".parse().unwrap(),
    );
    headers.insert("x-session-id", uuid_v4().parse().unwrap());
    headers
}

fn uuid_v4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = std::process::id();
    format!("taste-{nanos:016x}{pid:08x}")
}

// --- Main ---

#[tokio::main]
async fn main() {
    // MCP server runs on stdio — read JSON-RPC requests, respond on stdout
    loop {
        match read_jsonrpc() {
            None => break,
            Some(req) => {
                let id = req.get("id").cloned();
                let method = req
                    .get("method")
                    .and_then(|m| m.as_str())
                    .unwrap_or("");

                match method {
                    "initialize" => {
                        write_jsonrpc(
                            id,
                            json!({
                                "protocolVersion": "2024-11-05",
                                "capabilities": {"tools": {}},
                                "serverInfo": {"name": "cmdcode-taste", "version": "0.1.0"}
                            }),
                        );
                    }
                    "notifications/initialized" => {
                        // Acknowledgment — no response needed
                    }
                    "tools/list" => {
                        write_jsonrpc(id, json!({"tools": [taste_tool_schema()]}));
                    }
                    "tools/call" => {
                        let params = req.get("params").cloned().unwrap_or(json!({}));
                        let name = params
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("");

                        if name != "taste" {
                            write_jsonrpc_error(id, -32601, format!("Unknown tool: {name}"));
                            continue;
                        }

                        let args = params.get("arguments").cloned().unwrap_or(json!({}));
                        let instruction = args
                            .get("instruction")
                            .and_then(|i| i.as_str())
                            .unwrap_or("");

                        match handle_taste_call(instruction).await {
                            Ok(msg) => {
                                write_jsonrpc(
                                    id,
                                    json!({"content": [{"type": "text", "text": msg}]}),
                                );
                            }
                            Err(e) => {
                                write_jsonrpc_error(id, -32603, e);
                            }
                        }
                    }
                    _ => {
                        write_jsonrpc_error(id, -32601, format!("Method not found: {method}"));
                    }
                }
            }
        }
    }
}
