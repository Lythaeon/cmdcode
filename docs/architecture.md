# Architecture

## What is this?

A local HTTP proxy that speaks the **OpenAI chat completions API** on the
client side and translates to **Command Code's proprietary wire protocol** on
the upstream side. Written in Rust using Pingora for production-grade
performance.

```
┌──────────────────────┐         ┌──────────────────┐         ┌─────────────────────────────┐
│  OpenCode / LiteLLM  │  POST   │  command-code-   │  POST   │  api.commandcode.ai         │
│  / curl / any OA     │ ──────> │  proxy           │ ──────> │  /alpha/generate            │
│  compatible client   │ <────── │  (localhost:     │ <────── │  (NDJSON stream)            │
│                      │  SSE    │   18080)         │  NDJSON │                             │
└──────────────────────┘         └──────────────────┘         └─────────────────────────────┘
```

## Why?

Command Code's API uses a **custom wire protocol** that differs from OpenAI's
standard `/v1/chat/completions`. The official `cmd` CLI is the only
supported client. This proxy:

1. **Breaks vendor lock-in** - use any OpenAI-compatible toolchain
2. **Preserves API fingerprint** - sends the exact headers/body the CLI sends
   so the upstream cannot distinguish proxy traffic from the real CLI
3. **Production-grade** - Rust + Pingora for connection pooling, load balancing,
   and zero-copy I/O
4. **Streaming support** - full SSE translation for both text and tool calls

## Wire format translation

### Client → Proxy (OpenAI format)

```json
{
  "model": "gpt-5.6-luna",
  "messages": [
    {"role": "system", "content": "You are helpful."},
    {"role": "user", "content": "Hello"}
  ],
  "tools": [...],
  "stream": true,
  "max_tokens": 64000
}
```

### Proxy → Upstream (Command Code format)

```json
{
  "config": { "workingDir": "...", "date": "...", "isGitRepo": true, ... },
  "memory": null,
  "taste": null,
  "skills": null,
  "permissionMode": "standard",
  "mode": "agent",
  "params": {
    "model": "gpt-5.6-luna",
    "messages": [{"role": "user", "content": [{"type": "text", "text": "Hello"}]}],
    "tools": [{"name": "...", "description": "...", "input_schema": {...}}],
    "max_tokens": 64000,
    "stream": true
  }
}
```

### Headers (matches CLI fingerprint)

```
Content-Type: application/json
User-Agent: cli
x-command-code-version: <detected from installed CLI>
x-cli-environment: production
x-project-slug: <basename of cwd>
x-taste-learning: true
x-co-flag: false
x-session-id: <uuid per request>
Authorization: Bearer <api_key from ~/.commandcode/auth.json>
```

## Streaming translation

The upstream returns NDJSON (one JSON event per line). The proxy translates:

| Upstream event         | OpenAI SSE chunk                              |
|------------------------|-----------------------------------------------|
| `text-delta`           | `{"delta": {"content": "..."}}`               |
| `reasoning-delta`      | `{"delta": {"reasoning_content": "..."}}`     |
| `tool-call`            | `{"delta": {"tool_calls": [...]}}`            |
| `finish`               | `{"finish_reason": "stop" / "tool_calls"}`    |
| `error`                | `{"error": {"message": "..."}}`               |

## Message translation

OpenAI messages are converted to Command Code's content-array format:

- `"role": "user", "content": "text"` → `{"role": "user", "content": [{"type": "text", "text": "text"}]}`
- `"role": "assistant", "tool_calls": [...]` → `{"role": "assistant", "content": [{"type": "tool-call", ...}]}`
- `"role": "tool", "content": "result"` → `{"role": "tool", "content": [{"type": "tool-result", ...}]}`


