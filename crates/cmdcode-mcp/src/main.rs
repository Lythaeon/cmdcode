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
    // Route through the local proxy instead of upstream directly.
    // The upstream API detects non-CLI callers (TLS fingerprinting) and rejects them.
    // The local proxy already handles auth and upstream communication.
    std::env::var("COMMAND_CODE_PROXY_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:18080".into())
}

fn upstream_model() -> String {
    std::env::var("COMMAND_CODE_PROXY_DEFAULT")
        .unwrap_or_else(|_| "xiaomi/mimo-v2.5".into())
}

// --- MCP JSON-RPC server ---

fn eprintln(msg: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "[cmdcode-mcp] {msg}");
}

fn write_jsonrpc(id: Option<Value>, result: Value) {
    let resp = json!({"jsonrpc": "2.0", "result": result, "id": id});
    let serialized = serde_json::to_string(&resp).unwrap_or_default();
    eprintln(&format!("send: {serialized}"));
    let mut stdout = std::io::stdout();
    let _ = writeln!(stdout, "{serialized}");
    let _ = stdout.flush();
}

fn write_jsonrpc_error(id: Option<Value>, code: i64, message: String) {
    let resp = json!({"jsonrpc": "2.0", "error": {"code": code, "message": message}, "id": id});
    let serialized = serde_json::to_string(&resp).unwrap_or_default();
    eprintln(&format!("send(err): {serialized}"));
    let mut stdout = std::io::stdout();
    let _ = writeln!(stdout, "{serialized}");
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
        "model": upstream_model(),
        "messages": [
            {
                "role": "system",
                "content": "You are a taste learning assistant. Analyze user coding preferences and write taste files.\n\
                    You have access to a write_taste_file tool. Use it to record preferences.\n\
                    Format taste files as markdown with clear categories and rules.\n\
                    Only record genuinely stated preferences, don't invent them."
            },
            {
                "role": "user",
                "content": user_msg
            }
        ],
        "tools": [json!({
            "type": "function",
            "function": {
                "name": "write_taste_file",
                "description": "Write a taste category file. Use this to record preferences.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Path like 'style.md' or 'tools.md'"},
                        "content": {"type": "string", "description": "The content to write."}
                    },
                    "required": ["path", "content"]
                }
            }
        })],
        "max_tokens": 2048,
        "stream": false
    });

    // Call through local proxy
    let url = format!("{}/v1/chat/completions", upstream_url());
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    // Auth through proxy uses the proxy's incoming token, not the upstream key
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("Content-Type", "application/json".parse().unwrap());
    // The proxy accepts any token when no incoming token is configured
    headers.insert(
        reqwest::header::AUTHORIZATION,
        "Bearer command-code".parse().unwrap(),
    );

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

    // Parse OpenAI format response
    let response: Value = resp
        .json()
        .await
        .map_err(|e| format!("invalid upstream response: {e}"))?;

    // Extract tool calls from choices[0].message.tool_calls
    let tool_calls = response
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|msg| msg.get("tool_calls"))
        .and_then(|tc| tc.as_array())
        .cloned()
        .unwrap_or_default();

    let mut results = Vec::new();
    for tc in &tool_calls {
        let func = tc.get("function").cloned().unwrap_or(json!({}));
        let tool_name = func.get("name").and_then(|n| n.as_str()).unwrap_or("");
        let args_str = func.get("arguments").and_then(|a| a.as_str()).unwrap_or("{}");
        let input: Value = serde_json::from_str(args_str).unwrap_or(json!({}));

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

// --- Main ---

#[tokio::main]
async fn main() {
    eprintln(&format!("started, pid={}", std::process::id()));
    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());

    // MCP server runs on stdio — read JSON-RPC requests, respond on stdout
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => {
                eprintln("stdin closed, exiting");
                break;
            }
            Ok(_) => {}
            Err(e) => {
                eprintln(&format!("read error: {e}"));
                break;
            }
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            line.clear();
            continue;
        }
        eprintln(&format!("recv: {trimmed}"));

        let req: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                eprintln(&format!("parse error: {e}"));
                continue;
            }
        };

        let id = req.get("id").cloned();
        let method = req
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or("");
        eprintln(&format!("method: {method}"));

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
            "ping" => {
                write_jsonrpc(id, json!({}));
            }
            "notifications/cancelled" | "notifications/progress" => {
                // Acknowledgment — no response needed
            }
            "resources/list" => {
                write_jsonrpc(id, json!({"resources": []}));
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
        line.clear();
    }
}
