# cmdcode

OpenAI-compatible proxy in front of Command Code's HTTP API. Use your
Command Code subscription with **any** client supporting OpenAI Chat
Completions - no vendor lock-in.

## What it does

```
┌──────────────────┐       ┌───────────────┐       ┌─────────────────────────┐
│  OpenCode        │ POST  │  proxy        │ POST  │  Command Code API       │
│  LiteLLM         │ ────> │  :18080       │ ────> │  /alpha/generate        │
│  curl / any SDK  │ <──── │  (local)      │ <──── │  (NDJSON)               │
│                  │  SSE  │               │ NDJSON│                         │
└──────────────────┘       └───────────────┘       └─────────────────────────┘
```

The proxy translates the **OpenAI `/v1/chat/completions`** protocol to
Command Code's custom wire format, preserving the exact API fingerprint
(headers, body structure, auth) so the upstream cannot distinguish proxy
traffic from the official `cmd` CLI.

## Quick start

### Prerequisites

**No external CLI required.** The model catalog is bundled in the binary. Taste
files live at `~/.commandcode/taste/taste.md` (created by the agent during
conversations, or managed via `cmdcode taste`).

### Install

```bash
# From crates.io (requires Rust 1.75+)
cargo install cmdcode

# Or build from source
git clone https://github.com/Lythaeon/cmdcode.git
cd cmdcode
cargo install --path crates/cmdcode-cli
```

### Sign in

```bash
cmdcode auth
```

This opens an interactive TUI where you can:
- **Sign in a new account** — prints a Studio auth link; after signing in
  in the browser, the Studio POSTs your API key back to the local callback
  server automatically. You can also paste a key directly.
- **Switch active account** — the proxy reads the active credential on every
  request refresh (no restart needed).
- **Log out** — remove one or more accounts from the vault.

Credentials are backed up to `~/.cmdcode/accounts.json` (chmod 0600). The
proxy's `AuthManager` reads the active account from this vault, falling
back to the legacy `~/.commandcode/auth.json` if the vault is empty.

### Run

```bash
cmdcode serve
# listening on http://127.0.0.1:18080
```

### Test

```bash
curl http://127.0.0.1:18080/health
curl http://127.0.0.1:18080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"xiaomi/mimo-v2.5","messages":[{"role":"user","content":"Hello"}]}'
```

## Commands

| Command | Description |
|---------|-------------|
| `cmdcode serve` | Start the proxy server |
| `cmdcode auth` | Interactive TUI: manage accounts (list/use/logout/add, auto-rotate toggle) |
| `cmdcode status` | Check auth and model catalog status |
| `cmdcode models` | List available models |
| `cmdcode config` | Show current configuration |
| `cmdcode test` | Send a test request to verify proxy |
| `cmdcode setup` | Configure client harnesses |

## Multi-account & auto-rotate

Store multiple Command Code accounts and switch between them without
restarting the proxy:

```bash
cmdcode auth                    # TUI: list, use, logout, add
cmdcode auth use                # switch active account (TUI select)
cmdcode auth logout             # remove accounts (TUI multi-select)
```

Enable auto-rotate to switch accounts automatically when one hits its
credit limit or is rejected:

```bash
cmdcode auth                    # select "Auto-rotate: ON" in the TUI
```

The proxy reads the active credential from `~/.cmdcode/accounts.json` on
each TTL refresh — switching accounts takes effect within seconds without
a restart. When auto-rotate is enabled and the upstream returns 401/403/429
(credit exhausted / rate limit), the proxy rotates to the next account and
retries the request.

## Taste MCP Server

A standalone MCP server (`cmdcode-mcp`) exposes the `taste` tool for agents,
replicating command-code's built-in taste learning — fully decoupled from the
command-code CLI.

Add to your opencode config (`~/.config/opencode/opencode.json`):

```jsonc
{
  "mcp": {
    "cmdcode-taste": {
      "type": "local",
      "command": ["/path/to/cmdcode-mcp"],
      "enabled": true
    }
  }
}
```

The server reads `~/.commandcode/taste/taste.md` and calls the upstream API
(free — no credit cost) to analyze instructions. Results are written back
to the same taste files. Agents call the `taste` tool the same way they
would in command-code.

**Usage:**
```bash
# Build
cargo build --release -p cmdcode-mcp

# Test MCP handshake
echo '{"jsonrpc":"2.0","method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0.1.0"}},"id":1}' | ./target/release/cmdcode-mcp
```

## Features

- **Five protocol frontends** — OpenAI `/v1/chat/completions`,
  Anthropic `/v1/messages`, Gemini `:generateContent`,
  OpenAI Responses `/v1/responses` (with server-side `previous_response_id`
  chaining), and Ollama-native `/api/chat` — every one works against every
  configured upstream
- **Multi-provider routing** — declarative `providers.json`, hot reload,
  runtime enable/disable via `cmdcode connect`, per-model routing with
  default fallback
- **Google Gemini** — `:generateContent` / `:streamGenerateContent`
- **OpenAI Responses API** — `/v1/responses` (stateless subset)
- **Ollama-native** — `/api/chat` + `/api/tags`
- **Streaming** — full SSE translation (text, reasoning, tool calls)
- **Tool calls** — OpenAI function calling ↔ Command Code tool-call format
- **Multi-account auth** — vault at `~/.cmdcode/accounts.json` with TUI
  management, auto-rotate on credit limits, no proxy restart needed
- **Rust + Pingora** — production-grade, sub-millisecond overhead
- **CLI fingerprint** — sends the exact headers/body the `cmd` CLI sends
- **Auto-discovery** — model catalog parsed from CLI's bundled `models.md`
- **Reasoning effort** — `low`/`medium`/`high`/`xhigh`/`max` support
- **Retry logic** — automatic retry on transient upstream failures (502/503/504)
- **Health check** — `GET /health` for monitoring
- **CORS** — optional CORS headers for browser clients
- **Concurrency** — Pingora handles multiple simultaneous agents
- **Rate limiting** — configurable per-key rate limits (local or Redis backend)
- **Security** — zeroize, newtypes, constant-time comparison, CRLF sanitization
- **Harness auto-detection** — OpenCode, Codex, Hermes, LiteLLM, Ollama, vLLM, Open WebUI
- **Fuzz targets** — 14 fuzz targets covering wire format, auth, rate limiting, and harness types

## Configuration

| Env var | Default | Description |
|---------|---------|-------------|
| `COMMAND_CODE_PROXY_PORT` | `18080` | Listen port |
| `COMMAND_CODE_PROXY_HOST` | `127.0.0.1` | Bind address |
| `COMMAND_CODE_API_BASE` | `https://api.commandcode.ai` | Upstream API |
| `COMMAND_CODE_PROXY_TIMEOUT` | `600` | Upstream timeout (seconds) |
| `COMMAND_CODE_PROXY_RETRIES` | `2` | Retry count for transient failures |
| `COMMAND_CODE_PROXY_CORS` | (unset) | CORS origin header |
| `COMMAND_CODE_PROXY_DEFAULT` | `xiaomi/mimo-v2.5` | Default model |
| `COMMAND_CODE_PROXY_MODELS` | (unset) | Comma-separated model allowlist |
| `COMMAND_CODE_AUTH_DIR` | `~/.commandcode` | Legacy auth directory (vault at `~/.cmdcode` is preferred) |
| `COMMAND_CODE_ACCOUNTS_FILE` | `~/.cmdcode/accounts.json` | Multi-account vault path |
| `COMMAND_CODE_PROXY_LOG_FILE` | (unset) | Log file with size-based rotation |
| `COMMAND_CODE_PROXY_LOG_MAX_BYTES` | `52428800` | Rotate after this many bytes |
| `COMMAND_CODE_PROXY_LOG_KEEP` | `5` | Rotated log backups to keep |
| `COMMAND_CODE_PROXY_INCOMING_TOKEN` | (unset) | Require bearer token on API routes |
| `COMMAND_CODE_PROXY_TLS_CERT` | (unset) | TLS cert path (with KEY enables HTTPS) |
| `COMMAND_CODE_PROXY_TLS_KEY` | (unset) | TLS key path |
| `COMMAND_CODE_PROXY_RATE_LIMIT_MAX` | `100` | Max requests per window per key (0 = unlimited) |
| `COMMAND_CODE_PROXY_RATE_LIMIT_WINDOW` | `60` | Rate limit window in seconds |
| `COMMAND_CODE_PROXY_RATE_LIMIT_BACKEND` | `local` | Rate limit backend (`local` or `redis`) |
| `COMMAND_CODE_PROXY_RATE_LIMIT_REDIS_URL` | (unset) | Redis URL for distributed rate limiting |

## Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/v1/models` | List available models |
| `GET` | `/health` | Health check (status, version, upstream) |
| `GET` | `/metrics` | Prometheus-formatted metrics |
| `POST` | `/v1/chat/completions` | OpenAI chat completion (stream + non-stream) |
| `POST` | `/v1/messages` | Anthropic messages API (stream + non-stream) |
| `POST` | `/v1/responses` | OpenAI Responses API (stateless) |
| `POST` | `…:generateContent` / `…:streamGenerateContent` | Google Gemini |
| `GET`/`POST` | `/api/tags`, `/api/chat` | Ollama native |

See [Setup guide](docs/setup.md) for supervision, log rotation, and soak tooling.

## Wire it into OpenCode

Add to your `opencode.json`:

```jsonc
{
  "provider": {
    "command-code": {
      "npm": "@ai-sdk/openai-compatible",
      "name": "Command Code",
      "options": {
        "baseURL": "http://localhost:18080/v1"
      },
      "models": {
        "xiaomi/mimo-v2.5": { "name": "MiMo V2.5", "reasoning": true },
        "gpt-5.6-luna": { "name": "GPT-5.6 Luna", "reasoning": true }
      }
    }
  },
  "model": "default"
}
```

## Wire it into anything

**Python:** `OpenAI(base_url="http://127.0.0.1:18080", api_key="not-needed")`

**Node.js:** `new OpenAI({ baseURL: "http://127.0.0.1:18080", apiKey: "not-needed" })`

**LiteLLM:** `api_base: http://127.0.0.1:18080, api_key: not-needed`

## Documentation

- [Architecture](docs/architecture.md) — wire format translation details
- [Setup guide](docs/setup.md) — systemd, Docker, nginx, supervision
- [OpenCode integration](docs/opencode-integration.md) — step-by-step wiring

## License

MIT
