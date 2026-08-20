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

**You must have the `command-code` CLI installed and logged in.** It is a
hard dependency for two reasons:

1. **Auth** — the proxy reads your upstream credentials from the same place
   the `cmd` CLI does (`~/.commandcode/auth.json`), written by `cmd login`.
2. **Model catalog** — the proxy auto-discovers your model list from the CLI's
   bundled `models.md` (parsed into `/v1/models` with providers, reasoning
   effort levels, and context windows).

Install and log in:

```bash
npm install -g command-code
command-code login
```

Verify both artifacts exist:

```bash
ls ~/.commandcode/auth.json
ls ~/.linuxbrew/lib/node_modules/command-code/dist/bundled/command-code-knowledge/reference/models.md
```

If the CLI's `models.md` is not found at a standard location, you can point the
proxy at it explicitly with `COMMAND_CODE_PROXY_MODELS_CATALOG=/path/to/models.md`.
With neither, the proxy still serves requests but `/v1/models` returns empty.

> **Getting new models:** the catalog is read once at proxy startup from the
> CLI's bundled `models.md`. To see new models under `/v1/models`, update the
> CLI (`npm update -g command-code`) and restart the proxy. There is no
> runtime model discovery.

### Install

```bash
# Install from crates.io (requires Rust 1.75+)
cargo install cmdcode

# Or build from source
git clone https://github.com/Lythaeon/cmdcode.git
cd cmdcode
cargo install --path crates/cmdcode-cli
```

### Run

```bash
cmdcode
# listening on http://127.0.0.1:18080
```

If you want to require a bearer token on API routes (recommended for
non-localhost exposure), set `COMMAND_CODE_PROXY_INCOMING_TOKEN` before
starting:

```bash
COMMAND_CODE_PROXY_INCOMING_TOKEN=my-secret-token cmdcode
```

### Test

```bash
# Health check (always unauthenticated)
curl http://127.0.0.1:18080/health

# List models (unauthenticated unless a token is set)
curl http://127.0.0.1:18080/v1/models

# Send a completion
# If COMMAND_CODE_PROXY_INCOMING_TOKEN is set, add:
#   -H "Authorization: Bearer $COMMAND_CODE_PROXY_INCOMING_TOKEN"
curl http://127.0.0.1:18080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"xiaomi/mimo-v2.5","messages":[{"role":"user","content":"Hello"}]}'
```

`/health`, `/metrics`, and `/v1/models` are always served unauthenticated
for monitors and scrapers. When `COMMAND_CODE_PROXY_INCOMING_TOKEN` is set,
every other route requires `Authorization: Bearer <token>`.

## Features

- **OpenAI-compatible** - `/v1/chat/completions` (stream + non-stream)
- **Streaming** - full SSE translation (text, reasoning, tool calls)
- **Tool calls** - OpenAI function calling ↔ Command Code tool-call format
- **Rust + Pingora** - production-grade, sub-millisecond overhead
- **CLI fingerprint** - sends the exact headers/body the `cmd` CLI sends
- **Auto-discovery** - model catalog parsed from CLI's bundled `models.md`
- **Reasoning effort** - `low`/`medium`/`high`/`xhigh`/`max` support
- **Retry logic** - automatic retry on transient upstream failures (502/503/504)
- **Health check** - `GET /health` for monitoring
- **CORS** - optional CORS headers for browser clients
- **Concurrency** - Pingora handles multiple simultaneous agents

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

Restart OpenCode and you're done.

## Wire it into anything

Any OpenAI-compatible client works:

**Python:**
```python
from openai import OpenAI
client = OpenAI(base_url="http://127.0.0.1:18080", api_key="not-needed")
```

**Node.js:**
```javascript
import OpenAI from "openai";
const client = new OpenAI({ baseURL: "http://127.0.0.1:18080", apiKey: "not-needed" });
```

**LiteLLM:**
```yaml
model_list:
  - model_name: command-code
    litellm_params:
      model: openai/xiaomi/mimo-v2.5
      api_base: http://127.0.0.1:18080
      api_key: not-needed
```

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
| `COMMAND_CODE_AUTH_DIR` | `~/.commandcode` | Directory containing `auth.json` (override when auth lives elsewhere) |
| `COMMAND_CODE_PROXY_LOG_FILE` | (unset) | Log file with size-based rotation |
| `COMMAND_CODE_PROXY_LOG_MAX_BYTES` | `52428800` | Rotate after this many bytes |
| `COMMAND_CODE_PROXY_LOG_KEEP` | `5` | Rotated log backups to keep |
| `COMMAND_CODE_PROXY_INCOMING_TOKEN` | (unset) | Require bearer token on API routes |
| `COMMAND_CODE_PROXY_TLS_CERT` | (unset) | TLS cert path (with KEY enables HTTPS) |
| `COMMAND_CODE_PROXY_TLS_KEY` | (unset) | TLS key path |

## Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/v1/models` | List available models |
| `GET` | `/health` | Health check (status, version, upstream) |
| `GET` | `/metrics` | Prometheus-formatted metrics |
| `POST` | `/v1/chat/completions` | Chat completion (stream + non-stream) |

See `docs/setup.md` for supervision (`scripts/supervise.sh`), log rotation,
and soak (`scripts/soak.sh`) tooling.

## Performance

Proxy overhead over the direct upstream path, measured by the through-proxy
benchmark (`test_benchmark_through_proxy_vs_direct`): 500 requests against the
real Pingora proxy vs. the mock upstream, warm, on loopback.

```
direct : p50=0.427ms p95=0.469ms p99=0.519ms
proxy  : p50=0.804ms p95=0.991ms p99=1.247ms
overhead: p50=+0.377ms p95=+0.522ms p99=+0.728ms
```

The overhead is sub-millisecond at every percentile. These are machine-specific
warm-loopback numbers for reference; the regression test asserts a relative
bound (`p50 < direct + 2ms`, `p99 < direct + 10ms`) so CI noise, which affects
both paths equally, cancels out.

## Documentation

- [Architecture](docs/architecture.md) - wire format translation details
- [Setup guide](docs/setup.md) - installation, Docker, nginx
- [OpenCode integration](docs/opencode-integration.md) - step-by-step wiring

## License

MIT
