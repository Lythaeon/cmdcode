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

**You need the `command-code` CLI installed** — but only for the model
catalog (the proxy reads its bundled `models.md` to populate `/v1/models`).
You do **not** need to run `command-code login` — `cmdcode auth` handles
signing in via the Studio auth page.

```bash
npm install -g command-code
```

Verify the model catalog is accessible:

```bash
ls ~/.linuxbrew/lib/node_modules/command-code/dist/bundled/command-code-knowledge/reference/models.md
```

If the CLI's `models.md` is not found at a standard location, point the
proxy at it with `COMMAND_CODE_PROXY_MODELS_CATALOG=/path/to/models.md`.
Without it, the proxy serves requests but `/v1/models` returns empty.

> **Getting new models:** the catalog is read once at proxy startup from the
> CLI's bundled `models.md`. Update the CLI (`npm update -g command-code`)
> and restart the proxy to pick up new models.

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

## Features

- **OpenAI-compatible** — `/v1/chat/completions` (stream + non-stream)
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
| `POST` | `/v1/chat/completions` | Chat completion (stream + non-stream) |

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
