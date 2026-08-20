# OpenCode Integration

This guide shows how to wire the proxy into [OpenCode](https://opencode.ai)
so it uses your Command Code subscription as a native model provider.

## How it works

OpenCode supports any OpenAI-compatible API endpoint via the
`@ai-sdk/openai-compatible` provider. The proxy runs locally and presents
itself as one, translating requests to Command Code's API.

```
OpenCode ──> localhost:18080/v1/chat/completions ──> Command Code API
```

## Step 1: Start the proxy

```bash
cd /path/to/cmdcode
cargo run --release
# listening on http://127.0.0.1:18080
```

Or use systemd (see [setup.md](setup.md)).

## Step 2: Configure OpenCode

Add a custom provider in your `opencode.json`:

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

**Key fields:**
- `npm`: Must be `@ai-sdk/openai-compatible` (OpenCode's OpenAI-compatible provider)
- `options.baseURL`: Points to the proxy at `http://localhost:18080/v1`
  (the `/v1` suffix is required - OpenCode appends `/chat/completions`)
- `models`: Each key is a model ID that the proxy accepts

## Step 3: Restart OpenCode

```bash
# Restart to pick up the new config
opencode
```

## Using with agents

Assign the proxy model to specific agents in your `opencode.json`:

```jsonc
{
  "agent": {
    "sc-finder": {
      "model": "default"
    },
    "bounty-hunter": {
      "model": "default"
    }
  }
}
```

Or use a specific model directly:

```jsonc
{
  "agent": {
    "sc-finder": {
      "model": "command-code/xiaomi/mimo-v2.5"
    }
  }
}
```

## Available models

The proxy auto-discovers models from the installed `command-code` CLI.
Common models include:

| Model ID | Description |
|----------|-------------|
| `xiaomi/mimo-v2.5` | MiMo V2.5 (reasoning) |
| `gpt-5.6-luna` | GPT-5.6 Luna (reasoning) |
| `gpt-5.6-sol` | GPT-5.6 Sol (reasoning) |
| `deepseek/deepseek-v4-pro` | DeepSeek V4 Pro (reasoning) |
| `claude-sonnet-5` | Claude Sonnet 5 (reasoning) |

Models above your subscription plan are rejected (403) by the upstream API.

## Reasoning effort

Control reasoning depth with the `reasoning_effort` parameter or
model:effort syntax:

```json
{
  "model": "claude-sonnet-5:high",
  "messages": [{"role": "user", "content": "Analyze this code"}]
}
```

Effort levels: `low`, `medium`, `high`, `xhigh`, `max`

## Wire in any OpenAI-compatible harness

The proxy works with **any** tool that speaks the OpenAI chat completions API:

### LiteLLM

```yaml
model_list:
  - model_name: command-code
    litellm_params:
      model: openai/xiaomi/mimo-v2.5
      api_base: http://127.0.0.1:18080
      api_key: not-needed
```

### curl / HTTP clients

```bash
curl http://127.0.0.1:18080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "xiaomi/mimo-v2.5",
    "messages": [{"role": "user", "content": "Hello"}],
    "stream": true
  }'
```

### Python (openai SDK)

```python
from openai import OpenAI

client = OpenAI(
    base_url="http://127.0.0.1:18080",
    api_key="not-needed",
)

response = client.chat.completions.create(
    model="xiaomi/mimo-v2.5",
    messages=[{"role": "user", "content": "Hello"}],
)
print(response.choices[0].message.content)
```

### Node.js (openai SDK)

```javascript
import OpenAI from "openai";

const client = new OpenAI({
  baseURL: "http://127.0.0.1:18080",
  apiKey: "not-needed",
});

const response = await client.chat.completions.create({
  model: "xiaomi/mimo-v2.5",
  messages: [{ role: "user", content: "Hello" }],
});
console.log(response.choices[0].message.content);
```

## Why not use the official CLI directly?

The official `cmd` CLI is a monolithic Node.js harness. The proxy gives you:

1. **Composability** - plug into any OpenAI-compatible pipeline
2. **Multi-tenant** - run one proxy, serve multiple tools
3. **Observability** - standard HTTP logs, easy to proxy through nginx
4. **No vendor lock-in** - swap to any OpenAI-compatible provider by changing
   one URL
5. **Performance** - Rust + Pingora for sub-millisecond overhead
