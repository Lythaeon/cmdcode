# cmdcode

A local, multi-provider LLM gateway with taste injection. Speak five
protocols in, route to any number of upstream providers out — Command
Code, OpenAI-compatible endpoints, native Anthropic and native Gemini —
with hot-reloadable config and no vendor lock-in.

## What it does

```
                five protocols in                     upstream adapters
┌─────────────────────────────────┐   ┌──────────────────────────────────┐
│ /v1/chat/completions  (OpenAI)  │   │ command-code  (CLI fingerprint)  │
│ /v1/messages          (Anthropic)│   │ openai        (any compatible)   │
│ …:generateContent     (Gemini)  │ ─>│ anthropic     (native Messages)  │
│ /v1/responses         (Responses)│  │ gemini        (native generate)  │
│ /api/chat, /api/tags  (Ollama)  │   └────────────┬─────────────────────┘
└─────────────────────────────────┘                │
                       taste injection · rate limiting · session store
                                                   ▼
                                    providers.json — any mix, hot reload
```

Providers are declared in `~/.cmdcode/providers.json`, mirroring opencode's
provider map. Models route to the provider that declares them; undeclared
models fall back to the first enabled entry. Edits apply on the next
request — no restart.

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
| `cmdcode connect` | Interactive TUI: manage upstream providers (add/enable/disable/remove/test) |
| `cmdcode connect add` / `remove <id>` / `enable <id>` / `disable <id>` / `test <id>` / `list` | Non-interactive provider management |
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
a restart. When auto-rotate is enabled and the upstream rejects the
account (401/403/429, **or** a 400 "insufficient credits" response),
the proxy rotates to the next account and retries the request.

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

The server reads taste files from `~/.commandcode/taste/` and sends learning
requests to whichever upstream is configured for it: by default the
Command Code `/alpha/generate` endpoint (**free — no credit cost**); if your
`providers.json` flags an entry with `"learning": true`, that provider is
used instead (OpenAI-compatible chat completions). Results are written back
to the same taste files.

The MCP binary also reads `CMDCODE_PROVIDERS_CONFIG` to point at an
alternative providers config.

**Usage:**
```bash
# Build
cargo build --release -p cmdcode-mcp

# Test MCP handshake
echo '{"jsonrpc":"2.0","method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0.1.0"}},"id":1}' | ./target/release/cmdcode-mcp
```

## Features

- **Multi-provider upstreams** — four adapter types: `command-code` (CLI
  fingerprint, vault auth + rotation), `openai` (any compatible endpoint),
  `anthropic` (native Messages API), `gemini` (native generateContent)
- **Five protocol frontends** — OpenAI `/v1/chat/completions`,
  Anthropic `/v1/messages`, Gemini `:generateContent[:stream]`,
  OpenAI Responses `/v1/responses`, Ollama-native `/api/chat` — every
  frontend works against every configured upstream
- **Responses session store** — server-side `previous_response_id` chaining
  with TTL + entry cap; entries stored only after confirmed completion
- **Hot reload** — provider edits apply on the next request; broken config
  files retain the last-good router
- **Runtime enable/disable** — `"enabled": false` per provider; toggled via
  CLI or TUI; all-disabled yields a clean 503
- **Terminal-chunk dedup** — duplicate upstream terminal events never reach
  the client (streaming validators reject them)
- **Streaming** — full SSE/NDJSON translation: text, reasoning/thinking,
  tool-call deltas across every frontend/upstream pair
- **Tool calls** — function calling translated across all protocol
  combinations (nested ↔ flat schemas, tool_result ↔ tool role)
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
| `COMMAND_CODE_API_BASE` | `https://api.commandcode.ai` | Command Code API base |
| `CMDCODE_PROVIDERS_CONFIG` | `~/.cmdcode/providers.json` | Providers map path |
| `COMMAND_CODE_PROXY_PROVIDER` | `command-code` | Env-only adapter (`command-code` or `openai`) |
| `COMMAND_CODE_UPSTREAM_API_KEY` | (unset) | Bearer key for the env-only openai adapter |
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

## Providers

Declared in `~/.cmdcode/providers.json` (override with
`CMDCODE_PROVIDERS_CONFIG`). Each entry supports:

| Field | Description |
|-------|-------------|
| `type` | Adapter: `command-code`, `openai`, `anthropic`, or `gemini` |
| `options.baseURL` | Upstream base URL (adapter-specific default if omitted) |
| `options.apiKey` | API key — `{env:VAR}` interpolation supported |
| `models` | Model ids this provider serves (routes + `/v1/models` listing) |
| `learning` | Serve taste-learning requests from the MCP server |
| `enabled` | `false` removes it from routing without deleting the entry |

The first enabled entry is the fallback for undeclared models. Edits apply
on the next request (hot reload); a broken config file keeps the last-good
router. Manage entries with `cmdcode connect` instead of hand-editing.

## Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/v1/models` | List available models |
| `GET` | `/health` | Health check (status, version, upstream) |
| `GET` | `/metrics` | Prometheus-formatted metrics |
| `POST` | `/v1/chat/completions` | OpenAI chat completion (stream + non-stream) |
| `POST` | `/v1/messages` | Anthropic messages API (stream + non-stream) |
| `POST` | `/v1/responses` | OpenAI Responses API (with `previous_response_id` session chaining) |
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
