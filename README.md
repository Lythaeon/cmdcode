# command-code-openapi-proxy

OpenAI-compatible proxy in front of Command Code's HTTP API. Use your
Command Code subscription with **any** OpenAI-compatible toolchain —
no vendor lock-in.

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

```bash
# Prerequisites: Python 3.8+, command-code CLI authenticated

git clone https://github.com/Lythaeon/command-code-openapi-proxy.git
cd command-code-openapi-proxy

# Run
python3 -m command_code_proxy.proxy
# listening on http://127.0.0.1:18080

# Test
curl http://127.0.0.1:18080/v1/models
curl http://127.0.0.1:18080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"gpt-5.6-luna","messages":[{"role":"user","content":"Hello"}]}'
```

## Features

- **OpenAI-compatible** — `/v1/chat/completions` (stream + non-stream)
- **Streaming** — full SSE translation (text, reasoning, tool calls)
- **Tool calls** — OpenAI function calling ↔ Command Code tool-call format
- **Zero dependencies** — Python 3.8+ stdlib only
- **CLI fingerprint** — sends the exact headers/body the `cmd` CLI sends
- **Self-updating** — detects and installs newer CLI versions automatically
- **Retry logic** — automatic retry on transient upstream failures (502/503/504)
- **Health check** — `GET /health` for monitoring
- **CORS** — optional CORS headers for browser clients
- **Concurrency** — `ThreadingHTTPServer` handles multiple simultaneous agents
- **Structured logging** — request IDs, latency, upstream status

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
        "gpt-5.6-luna": { "name": "GPT-5.6 Luna", "reasoning": true },
        "xiaomi/mimo-v2.5-pro": { "name": "MiMo V2.5 Pro", "reasoning": true }
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
      model: openai/gpt-5.6-luna
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

## Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/v1/models` | List available models |
| `GET` | `/health` | Health check (status, version, upstream) |
| `POST` | `/v1/chat/completions` | Chat completion (stream + non-stream) |

## Install as systemd service

```bash
cp systemd/command-code-proxy.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now command-code-proxy
```

## Documentation

- [Architecture](docs/architecture.md) — wire format translation details
- [Setup guide](docs/setup.md) — installation, Docker, nginx
- [OpenCode integration](docs/opencode-integration.md) — step-by-step wiring

## License

MIT
