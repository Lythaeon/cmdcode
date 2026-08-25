# Architecture

## What is this?

A local, multi-provider LLM gateway written in Rust using Pingora. It
speaks **five client protocols** on the frontend and routes to **any mix of
configured upstream providers** through a hot-reloadable router:

```
                 five frontends                     provider adapters
┌─────────────────────────────┐    ┌──────────────────┐    ┌──────────────────────────┐
│ /v1/chat/completions (OA)   │    │                  │    │ command-code             │
│ /v1/messages       (Anthro) │ ─> │ Provider Router  │ ─> │ openai-compatible        │
│ :generateContent   (Gemini) │    │ (hot reload,     │    │ anthropic-native         │
│ /v1/responses      (Resp.)  │    │  per-model map)  │    │ gemini-native            │
│ /api/chat          (Ollama) │    │                  │    │ (providers.json)         │
└─────────────────────────────┘    └──────────────────┘    └──────────────────────────┘
```

Every request is normalized into an internal OpenAI-format representation;
frontend adapters convert inbound dialects into it and upstream adapters
convert it out again. This N×M problem is reduced to N+M adapters.

### Adapter contracts

**Frontends** (inbound): parse a protocol-specific body into
`ChatCompletionRequest`, render responses/streams back out in the same
dialect.

**Upstreams** (`Provider` trait): given the normalized request, supply

- `endpoint(model, streaming)` — full upstream URL
- `headers()` — identity/auth headers
- `build_body()` — provider wire format (with taste section injected)
- `translate_line()` — upstream stream line → OpenAI SSE chunk frames
- `parse_non_streaming()` — non-streaming body → OpenAI completion JSON
- `should_rotate()/on_auth_rejected()` — credential failure handling

The Command Code adapter preserves the exact CLI fingerprint so upstream
cannot distinguish proxy traffic from the official CLI.

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


