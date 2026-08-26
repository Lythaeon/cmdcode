//! Node.js TLS tunnel for the Anthropic adapter.
//!
//! When Anthropic subscription (OAuth) traffic is TLS-fingerprinted,
//! Rust's reqwest produces a different JA3/JA4 than Claude Code (which
//! runs on Node.js/OpenSSL). This tunnel spawns a real Node.js HTTPS
//! client as a local reverse proxy so the TLS handshake is byte-for-byte
//! identical to Claude Code's.

use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

/// Port allocator for tunnel instances.
static TUNNEL_PORT: AtomicU32 = AtomicU32::new(18981);

const TUNNEL_SCRIPT: &str = r#"
import http from 'http';
import https from 'https';
import { URL } from 'url';

const UPSTREAM = 'api.anthropic.com';
const server = http.createServer((req, res) => {
  const chunks = [];
  req.on('data', c => chunks.push(c));
  req.on('end', () => {
    const body = Buffer.concat(chunks);
    const headers = { ...req.headers };
    delete headers.host;
    delete headers.connection;

    const opts = {
      hostname: UPSTREAM,
      port: 443,
      path: req.url,
      method: req.method,
      headers,
    };
    const upstream = https.request(opts, (upRes) => {
      res.writeHead(upRes.statusCode, upRes.headers);
      upRes.pipe(res);
    });
    upstream.on('error', (e) => {
      res.writeHead(502, {'content-type': 'application/json'});
      res.end(JSON.stringify({error: {message: e.message}}));
    });
    if (body.length > 0) upstream.write(body);
    upstream.end();
  });
});
server.listen(parseInt(process.argv[2] || '18981'), '127.0.0.1');
"#;

/// A running Node.js TLS tunnel to api.anthropic.com on a local port.
pub struct TlsTunnel {
    child: Child,
    /// Local port the tunnel listens on.
    pub port: u16,
}

impl TlsTunnel {
    /// Spawn a new Node.js tunnel process listening on an ephemeral local
    /// port. Returns None if Node.js is not available.
    pub fn spawn() -> Option<Self> {
        let script_path = std::env::temp_dir().join("cmdcode-anthropic-tunnel.mjs");
        {
            let mut f = std::fs::File::create(&script_path).ok()?;
            f.write_all(TUNNEL_SCRIPT.as_bytes()).ok()?;
        }

        let port = TUNNEL_PORT.fetch_add(1, Ordering::Relaxed) as u16;

        // Find node — prefer claude's bundled binary, then system node.
        let node = find_node()?;

        let child = Command::new(&node)
            .arg(&script_path)
            .arg(port.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;

        Some(Self { child, port })
    }

    /// Base URL to use instead of `https://api.anthropic.com`.
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

impl Drop for TlsTunnel {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn find_node() -> Option<String> {
    for candidate in ["node", "/usr/bin/node", "/usr/local/bin/node"] {
        if Command::new(candidate).arg("--version").output().is_ok() {
            return Some(candidate.into());
        }
    }
    None
}
