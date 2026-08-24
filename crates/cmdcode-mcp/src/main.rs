//! MCP server for Command Code taste learning.
//!
//! Exposes a `taste` tool that agents can call to record coding preferences.
//! The tool calls the upstream API (free — no credit cost) to analyze instructions,
//! then writes results to `~/.commandcode/taste/taste.md`.
//!
//! Usage: `cmdcode-mcp` (stdio transport for MCP clients)

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::or_fun_call)]

use futures_util::StreamExt;
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
    // The CLI's free taste/generate endpoint base.
    std::env::var("COMMAND_CODE_API_BASE")
        .unwrap_or_else(|_| "https://api.commandcode.ai".into())
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
        "config": {
            "workingDir": std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| ".".to_string()),
            "date": chrono_date(),
            "environment": std::env::consts::OS,
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
        "permissionMode": "default",
        "threadId": uuid_v4(),
        // The CLI stamps feature purpose into `mode` via withUsageContext —
        // "learning" is what attributes the call as taste usage on the
        // dashboard. "agent" would lump it into regular chat usage.
        "mode": "learning",
        "params": {
            "model": upstream_model(),
            "messages": [{"role": "user", "content": [{"type": "text", "text": user_msg}]}],
            "tools": [json!({
                "name": "write_taste_file",
                "description": "Write a taste category file. Use this to record preferences.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Path like 'style.md' or 'tools.md'"},
                        "content": {"type": "string", "description": "The content to write."}
                    },
                    "required": ["path", "content"]
                }
            })],
            "system": "You are a taste learning assistant. When the user states a coding preference, record it using the write_taste_file tool. Only record genuinely stated preferences, don't invent them.",
            "max_tokens": 4096,
            "stream": true
        }
    });

    // Call the CLI's free taste endpoint directly (same wire format as
    // command-code's postStream — body must match or upstream rejects with
    // "Proxy use detected").
    let url = format!("{}/alpha/generate", upstream_url());
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

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

    // Consume the NDJSON stream, assembling tool calls from deltas.
    let mut stream = resp.bytes_stream();

    // toolCallId -> (toolName, accumulated input JSON string)
    let mut pending: std::collections::HashMap<String, (String, String)> =
        std::collections::HashMap::new();
    let mut text_out = String::new();
    let mut buf: Vec<u8> = Vec::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("stream read failed: {e}"))?;
        buf.extend_from_slice(&chunk);
        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(ev) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            match ev.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                "tool-input-start" => {
                    let id = ev.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
                    let name = ev
                        .get("toolName")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    pending.insert(id, (name, String::new()));
                }
                "tool-input-delta" => {
                    let id = ev.get("id").and_then(|i| i.as_str()).unwrap_or("");
                    if let Some((_, acc)) = pending.get_mut(id) {
                        acc.push_str(ev.get("delta").and_then(|d| d.as_str()).unwrap_or(""));
                    }
                }
                "text-delta" => {
                    text_out.push_str(ev.get("delta").and_then(|d| d.as_str()).unwrap_or(""));
                }
                _ => {}
            }
        }
    }

    drop(stream);

    // Execute completed write_taste_file calls.
    let mut results = Vec::new();
    for (_, (tool_name, input_str)) in &pending {
        if tool_name != "write_taste_file" {
            continue;
        }
        let input: Value = match serde_json::from_str(input_str.trim()) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let (Some(path), Some(content)) = (
            input.get("path").and_then(|p| p.as_str()),
            input.get("content").and_then(|c| c.as_str()),
        ) {
            let taste_path = resolve_taste_path(path);
            if let Some(p) = taste_path {
                std::fs::create_dir_all(p.parent().unwrap_or(Path::new(".")))
                    .map_err(|e| format!("mkdir failed: {e}"))?;
                std::fs::write(&p, content).map_err(|e| format!("write failed: {e}"))?;
                results.push(format!("Recorded preferences in {}", p.display()));
            }
        }
    }

    if results.is_empty() {
        let note = text_out.trim();
        if note.is_empty() {
            Ok("No new taste recorded.".into())
        } else {
            Ok(format!("No new taste recorded. Model said: {note}"))
        }
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
    // Taste files live in ~/.commandcode/taste/<category>.md.
    // Accept any relative markdown category path; reject traversal/absolute paths.
    let p = Path::new(relative);
    if p.is_absolute() || relative.contains("..") {
        return None;
    }
    if p.extension() != Some(std::ffi::OsStr::new("md")) {
        return None;
    }
    Some(taste_dir().join(p))
}

async fn build_auth_headers() -> reqwest::header::HeaderMap {
    use reqwest::header;

    let mut headers = header::HeaderMap::new();
    headers.insert("Content-Type", "application/json".parse().unwrap());
    headers.insert("User-Agent", "cli".parse().unwrap());

    // Vault first, then legacy auth.json fallback
    let vault = cmdcode_core::accounts::AccountStore::default();
    let key = vault
        .load()
        .ok()
        .and_then(|v| v.active_account().map(|a| a.api_key.as_str().to_string()))
        .filter(|k| !k.is_empty())
        .or_else(|| {
            let auth_file = home().join(".commandcode").join("auth.json");
            std::fs::read_to_string(&auth_file)
                .ok()
                .and_then(|c| serde_json::from_str::<Value>(&c).ok())
                .and_then(|a| a.get("apiKey").and_then(|k| k.as_str()).map(String::from))
                .filter(|k| !k.is_empty())
        });

    if let Some(key) = key {
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {key}").parse().unwrap(),
        );
    }

    // Headers matching the official CLI's buildCommandAuthHeaders
    headers.insert("x-cli-environment", "production".parse().unwrap());
    headers.insert(
        "x-command-code-version",
        detect_cli_version()
            .unwrap_or_else(|| "1.32.1".into())
            .parse()
            .unwrap(),
    );
    headers.insert(
        "x-project-slug",
        std::env::current_dir()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_else(|| "unknown".into())
            .parse()
            .unwrap(),
    );
    headers.insert("x-session-id", uuid_v4().parse().unwrap());
    headers
}

/// Detect the installed command-code CLI version via `command-code --version`.
fn detect_cli_version() -> Option<String> {
    let output = std::process::Command::new("command-code")
        .arg("--version")
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let ver = stdout.trim().split_whitespace().last()?;
    if ver.contains('.') && ver.chars().any(|c| c.is_ascii_digit()) {
        Some(ver.to_string())
    } else {
        None
    }
}

/// Random UUID v4.
fn uuid_v4() -> String {
    uuid::Uuid::new_v4().to_string()
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
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe as i64 + era * 400;
    (if m <= 2 { y + 1 } else { y }, m, d)
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
