use cmdcode_core::accounts::Account;
use cmdcode_core::types::SensitiveString;
use std::io::Read;
use std::net::TcpListener;

// --- Constants matching the official CLI ---

const START_PORT: u16 = 5959;
const MAX_PORT_ATTEMPTS: u16 = 10;
const CALLBACK_TIMEOUT_SECS: u64 = 120;

/// Studio CLI auth origin (official `buildCommandAuthUrl` base).
const STUDIO_AUTH_URL: &str = "https://commandcode.ai/studio/auth/cli";

/// CORS allowlist mirrored from the official CLI.
const ALLOWED_ORIGINS: [&str; 3] = [
    "http://localhost:3000",
    "https://staging.commandcode.ai",
    "https://commandcode.ai",
];

// --- State token generation ---

/// Generate an opaque anti-CSRF state token. Not crypto-strength — just needs
/// to be unpredictable enough for a localhost callback session.
fn generate_state_token() -> String {
    use std::hash::{Hash, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    nanos.hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    std::thread::current().id().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

// --- Port discovery ---

fn find_available_port() -> Option<u16> {
    (0..MAX_PORT_ATTEMPTS)
        .map(|i| START_PORT + i)
        .find(|p| TcpListener::bind(("127.0.0.1", *p)).is_ok())
}

// --- URL construction ---

/// Build the Studio auth URL exactly as `buildCommandAuthUrl` does.
fn build_auth_url(port: u16, state: &str) -> String {
    let callback = format!("http://127.0.0.1:{port}/callback");
    format!(
        "{STUDIO_AUTH_URL}?callback={}&state={}",
        encode_uri_component(&callback),
        encode_uri_component(state)
    )
}

/// Minimal percent-encoding matching `encodeURIComponent`.
/// Unreserved: A-Z a-z 0-9 - _ . ! ~ * ' ( )
fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

// --- Whoami validation (manual paste) ---

#[derive(Debug, serde::Deserialize)]
struct WhoamiResponse {
    user: WhoamiUser,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct WhoamiUser {
    id: String,
    #[serde(default)]
    user_name: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

/// Validate an API key against `GET /alpha/whoami` and build an account from it.
pub fn login_with_api_key(api_key: &str, upstream_url: &str) -> Result<Account, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;

    let res = client
        .get(format!("{upstream_url}/alpha/whoami"))
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {api_key}"))
        .send()
        .map_err(|e| format!("failed to reach upstream: {e}"))?;

    match res.status().as_u16() {
        200 => {
            let body: WhoamiResponse = res
                .json()
                .map_err(|e| format!("invalid whoami response: {e}"))?;
            Ok(Account {
                api_key: SensitiveString::new(api_key.to_string()),
                user_id: body.user.id,
                user_name: body.user.user_name.or(body.user.name).unwrap_or_default(),
                key_name: "cli-manual-entry".to_string(),
                authenticated_at: now_iso(),
                label: String::new(),
            })
        }
        401 => Err("invalid api key (401 unauthorized)".into()),
        code => Err(format!("validation failed with HTTP {code}")),
    }
}

// --- Studio callback login ---

/// Callback body the Studio POSTs to `http://127.0.0.1:{port}/callback`.
#[derive(Debug, serde::Deserialize)]
struct CallbackPayload {
    #[serde(alias = "apiKey")]
    api_key: SensitiveString,
    state: String,
    #[serde(alias = "userId")]
    user_id: String,
    #[serde(alias = "userName")]
    user_name: String,
    #[serde(alias = "keyName")]
    key_name: String,
}

/// Generate the Studio auth URL + state + port for the callback flow.
/// Does NOT start any server — call `run_callback_server` separately.
pub fn make_auth_url() -> Result<(u16, String, String), String> {
    let port = find_available_port().ok_or("no free port for callback server")?;
    let state = generate_state_token();
    let url = build_auth_url(port, &state);
    Ok((port, state, url))
}

/// Start the callback server on the given port and wait for the Studio POST.
/// Must be called from a thread since it blocks.
pub fn run_callback_server(port: u16, state: &str) -> Result<Account, String> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| format!("failed to start callback listener: {e}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("failed to set nonblocking: {e}"))?;

    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(CALLBACK_TIMEOUT_SECS);
    while std::time::Instant::now() < deadline {
        match listener.accept() {
            Ok((mut stream, _)) => {
                // Read the full HTTP request until \r\n\r\n (headers done).
                // The Studio callback sends the JSON body with Content-Length,
                // so the body follows immediately after the header separator.
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .ok();
                let mut buf = Vec::with_capacity(4096);
                let mut tmp = [0u8; 4096];
                loop {
                    match stream.read(&mut tmp) {
                        Ok(0) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&tmp[..n]);
                            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                                break;
                            }
                            if buf.len() > 1_000_000 {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }

                let request = String::from_utf8_lossy(&buf);

                // Only handle POST /callback and OPTIONS preflight.
                let is_options = request.starts_with("OPTIONS");
                let is_post_callback = request.starts_with("POST /callback");
                if !is_post_callback && !is_options {
                    let _ = send_response(
                        &mut stream,
                        404,
                        origin_of(&request),
                        Some(r#"{"error":"Not Found"}"#),
                    );
                    continue;
                }

                if is_options {
                    let _ = send_response(&mut stream, 204, origin_of(&request), None);
                    continue;
                }

                // Parse JSON body.
                let body = request.split("\r\n\r\n").nth(1).unwrap_or_default().trim();
                let payload: CallbackPayload = match serde_json::from_str(body) {
                    Ok(p) => p,
                    Err(e) => {
                        let _ = send_response(
                            &mut stream,
                            400,
                            origin_of(&request),
                            Some(&format!(r#"{{"error":"Invalid JSON: {e}"}}"#)),
                        );
                        continue;
                    }
                };

                if payload.state != state {
                    let _ = send_response(
                        &mut stream,
                        403,
                        origin_of(&request),
                        Some(r#"{"error":"State mismatch"}"#),
                    );
                    return Err("state mismatch — authorization failed".into());
                }

                let _ = send_response(
                    &mut stream,
                    200,
                    origin_of(&request),
                    Some(r#"{"success":true}"#),
                );

                let account = Account {
                    api_key: payload.api_key,
                    user_id: payload.user_id,
                    user_name: payload.user_name,
                    key_name: payload.key_name,
                    authenticated_at: now_iso(),
                    label: String::new(),
                };
                return Ok(account);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(_) => {
                return Err("listener error".into());
            }
        }
    }

    Err("timed out waiting for authorization".into())
}

/// Extract the `Origin` header from a raw HTTP request.
fn origin_of(request: &str) -> &str {
    request
        .lines()
        .find(|l| l.to_lowercase().starts_with("origin:"))
        .and_then(|l| l.split(':').nth(1))
        .map(|v| v.trim())
        .unwrap_or("https://commandcode.ai")
}

/// Reflect CORS `Access-Control-Allow-Origin` if the origin is in the
/// allowlist (mirrors `allowedCorsOrigin` from the official CLI).
fn cors_origin(origin: &str) -> &str {
    if ALLOWED_ORIGINS.contains(&origin) {
        origin
    } else {
        ALLOWED_ORIGINS[0]
    }
}

/// Send a minimal HTTP response.
fn send_response(
    stream: &mut impl std::io::Write,
    status: u16,
    origin: &str,
    body: Option<&str>,
) -> std::io::Result<()> {
    let status_text = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        _ => "OK",
    };
    let origin_hdr = cors_origin(origin);
    let body_bytes = body.unwrap_or("").as_bytes();
    let resp = format!(
        "HTTP/1.1 {status} {status_text}\r\n\
         Access-Control-Allow-Origin: {origin_hdr}\r\n\
         Access-Control-Allow-Methods: POST, OPTIONS\r\n\
         Access-Control-Allow-Headers: Content-Type\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n",
        body_bytes.len(),
    );
    stream.write_all(resp.as_bytes())?;
    stream.write_all(body_bytes)?;
    Ok(())
}

// --- Shared utilities ---

fn now_iso() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = now / 86400;
    let (y, m, d) = civil_from_days(days as i64);
    let secs = now % 86400;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_url_encode_basic() {
        assert!(encode_uri_component("abc-_.~*").contains("abc-_.~*"));
        assert!(encode_uri_component("a b?c=d").contains("%20"));
        assert!(encode_uri_component("a b?c=d").contains("%3F"));
    }

    #[test]
    fn test_find_available_port_returns_some() {
        let port = find_available_port();
        assert!(port.is_some(), "should find a free port");
        let p = port.unwrap();
        assert!(p >= START_PORT && p < START_PORT + MAX_PORT_ATTEMPTS);
    }

    #[test]
    fn test_build_auth_url_matches_cli() {
        let url = build_auth_url(5959, "test-state-123");
        assert!(url.starts_with("https://commandcode.ai/studio/auth/cli?callback="));
        assert!(url.contains("state=test-state-123"));
        assert!(url.contains("callback=http%3A%2F%2F127.0.0.1%3A5959%2Fcallback"));
    }

    #[test]
    fn test_callback_payload_parse() {
        let json =
            r#"{"apiKey":"user_abc","state":"abc","userId":"u1","userName":"alice","keyName":"k"}"#;
        let p: CallbackPayload = serde_json::from_str(json).unwrap();
        assert_eq!(p.api_key.as_str(), "user_abc");
        assert_eq!(p.user_id, "u1");
        assert_eq!(p.user_name, "alice");
    }

    #[test]
    fn test_now_iso_structure() {
        let s = now_iso();
        assert!(s.ends_with('Z'), "ISO should end with Z: {s}");
        assert!(s.contains('T'));
    }

    #[test]
    fn test_cors_origin_allowed() {
        assert_eq!(
            cors_origin("https://commandcode.ai"),
            "https://commandcode.ai"
        );
        assert_eq!(cors_origin("https://evil.com"), ALLOWED_ORIGINS[0]);
    }
}

/// Integration-style tests that exercise the callback and manual paths
/// against in-process mock servers.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod integration_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Mutex;
    use std::thread;

    /// Serialize all tests that bind to ports (port race prevention).
    static PORT_LOCK: Mutex<()> = Mutex::new(());

    /// Spin up a mock Studio that POSTs credentials to the callback port.
    fn mock_studio_post(port: u16, state: &str) {
        let state = state.to_owned();
        thread::spawn(move || {
            // Wait briefly for the callback server to be ready.
            std::thread::sleep(std::time::Duration::from_millis(300));
            let mut s = std::net::TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
            let body = serde_json::json!({
                "apiKey": "user_studio_key",
                "state": state,
                "userId": "u-studio",
                "userName": "studio_user",
                "keyName": "cli-studio",
            });
            let body_str = serde_json::to_string(&body).unwrap();
            let req = format!(
                "POST /callback HTTP/1.1\r\n\
                 Host: 127.0.0.1:{port}\r\n\
                 Origin: https://commandcode.ai\r\n\
                 Content-Type: application/json\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\r\n\
                 {body_str}",
                body_str.len()
            );
            let _ = s.write_all(req.as_bytes());
            // Read response
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
        });
    }

    #[test]
    fn test_callback_server_receives_post() {
        let _guard = PORT_LOCK.lock().unwrap();
        // Get a port, build state, launch mock Studio + callback server
        let port = find_available_port().expect("no free port");
        let state = "test-callback-state-abc";
        mock_studio_post(port, state);

        let acct = run_callback_server(port, state).expect("callback_login should succeed");
        assert_eq!(acct.api_key.as_str(), "user_studio_key");
        assert_eq!(acct.user_id, "u-studio");
        assert_eq!(acct.user_name, "studio_user");
        assert_eq!(acct.key_name, "cli-studio");
    }

    #[test]
    fn test_callback_state_mismatch() {
        let _guard = PORT_LOCK.lock().unwrap();
        let port = find_available_port().expect("no free port");
        // Send a wrong state value
        mock_studio_post(port, "wrong-state");
        let err = run_callback_server(port, "expected-state");
        assert!(err.is_err());
        assert!(err.unwrap_err().contains("mismatch"));
    }

    #[test]
    fn test_manual_paste_whoami_mock() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf);
            let payload = String::from_utf8_lossy(&buf);
            let ok = payload.contains("Bearer user_manual_key");
            let body = if ok {
                r#"{"user":{"id":"u-manual","userName":"manual_user"}}"#
            } else {
                r#"{"error":"unauthorized"}"#
            };
            let resp = format!(
                "HTTP/1.1 {}\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\nconnection: close\r\n\r\n{body}",
                if ok { 200 } else { 401 },
                body.len()
            );
            let _ = s.write_all(resp.as_bytes());
        });

        let acct = login_with_api_key("user_manual_key", &format!("http://{addr}")).unwrap();
        assert_eq!(acct.api_key.as_str(), "user_manual_key");
        assert_eq!(acct.user_id, "u-manual");
        assert_eq!(acct.user_name, "manual_user");

        let bad = login_with_api_key("user_bad", &format!("http://{addr}"));
        assert!(bad.is_err());
    }
}
