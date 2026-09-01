# Changelog

All notable changes to cmdcode are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/), and this project adheres to [Semantic Versioning](https://semver.org/).

## [0.5.0] - 2026-08-31

The OpenCode compatibility release. Fixes duplicate tool call ID errors
and context overflow detection so OpenCode's auto-compaction works
seamlessly through the proxy.

### Added
- **Request-side tool call deduplication** — `deduplicate_tool_calls()`
  on `ChatCompletionRequest` removes duplicate tool call IDs from
  assistant messages and duplicate tool-result messages before forwarding
  to upstream LLMs. Prevents "Duplicate value for 'tool_call_id'" errors
  from clients that accumulate duplicate entries in conversation history.
- **Response-side tool call deduplication** — streaming `tool-call` and
  `tool-input-end` events are deduplicated via `seen_tool_calls` HashSet
  on `StreamState`. Duplicate events from upstream are skipped with a
  counter, preventing client streaming validator failures.
- **Context overflow error detection** — upstream HTTP errors are parsed
  and inspected for context length indicators (`context_length_exceeded`
  code, "context length" / "token limit" / "exceeds the model's maximum
  context" in message). Matching errors are forwarded with
  `type: "invalid_request_error"` and `code: "context_length_exceeded"`
  so OpenCode can trigger auto-compaction instead of stopping the session.
- **Duplicate finish event suppression** — `finish` and `finish-step`
  events are deduplicated; at most one terminal chunk with `finish_reason`
  + usage is emitted per stream.

### Fixed
- **Tool result removal during deduplication** — the initial dedup
  implementation incorrectly marked the first tool-result message for
  removal instead of actual duplicates. Rewrote to use separate passes:
  first collect tool_call_ids from assistant messages, then remove
  duplicate tool-result messages.
- **Tool input index inflation** — `tool-input-start` now only increments
  `tool_index` when inserting a new entry (`or_insert_with`) instead of
  unconditionally, preventing index gaps when duplicate start events are
  received.

### Performance
- Tool call deduplication runs once per request with O(n) HashSet
  operations; no measurable overhead on typical conversation lengths.

**Compare**: https://github.com/Lythaeon/cmdcode/compare/v0.4.0...v0.5.0

## [0.4.0] - 2026-08-25

The provider-agnostic gateway release. cmdcode is no longer only a
Command Code proxy — it is a multi-provider, multi-protocol LLM gateway
with taste injection on every path.

### Added
- **Multi-provider upstream routing** — declare any number of upstreams in
  `~/.cmdcode/providers.json` (opencode-style providers map) with per-entry
  adapter type, base URL, `{env:VAR}`-interpolated API keys and model lists.
  Models route to the provider that declares them; undeclared models fall
  back to the first entry. `/v1/models` merges all providers with the
  bundled catalog.
- **Native Anthropic upstream** (`type: "anthropic"`) — `/v1/messages`
  protocol: system param, tool_use/tool_result blocks, flat tools,
  extended-thinking budget from reasoning effort, SSE event translation.
- **Native Gemini upstream** (`type: "gemini"`) — `:generateContent` /
  `:streamGenerateContent?alt=sse`: contents/parts, functionCall/
  functionResponse, functionDeclarations, generationConfig + thinkingConfig.
- **Anthropic frontend** — `POST /v1/messages` with typed content blocks
  (text/thinking/tool_use), `x-api-key` auth, full streaming event sequence
  (`message_start` → `content_block_*` → `message_delta` → `message_stop`).
- **Google Gemini frontend** — `:generateContent` / `:streamGenerateContent`.
- **OpenAI Responses API frontend** — `POST /v1/responses` (stateless subset)
  plus a server-side session store: responses chain via
  `previous_response_id` (1h TTL, 10k cap, stored only after confirmed
  completion). Server-assigned `resp_*` ids stamped into streamed events.
- **Ollama-native frontend** — `/api/chat` NDJSON streaming + `/api/tags`.
- **`cmdcode connect`** — manage providers from the CLI: interactive TUI
  (mirrors `auth`), plus non-interactive `add | list | enable | disable |
  remove | test`. Changes hot-reload; no proxy restart.
- **Hot reload** — the provider router stats the config per request and swaps
  atomically on mtime change. Broken or removed config files retain the
  last-good router instead of degrading.
- **Runtime enable/disable** — `"enabled": false` on any provider entry;
  disabled entries drop out of routing and `/v1/models`; all-disabled yields
  a clean 503.

### Fixed
- **Per-request subprocess spawn (~600ms tax)** — CLI version detection ran
  `command-code --version` on every upstream call; now cached process-wide.
  Concurrency-matrix p99 dropped from 30s timeouts to sub-millisecond.
- **Streaming routed to wrong translator** — the main stream loop hardcoded
  the Command Code translator, breaking all other adapters' streams.
- **Gemini tool-arg corruption** — argument fragments were emitted as text
  parts; now buffered and flushed as complete `functionCall` parts.
- **Taste cache staleness** — replaced an append-only cache that could serve
  outdated taste content after key changes.
- **Credit-exhaustion rotation** — command-code returns 400 "insufficient
  credits" (not 401/429); account rotation now triggers on it too.
- **Input validation** — Gemini empty `contents` / Ollama empty `messages`
  return protocol-shaped 400s instead of forwarding to upstream.
- Per-client rate-limit buckets now keyed by presented credential even when
  the proxy itself does not require auth.
- Session store inserts deferred to confirmed completion so failed
  generations do not pollute chained context.

### Security
- Control-character stripping on header-derived values (project slug)
  prevents CR/LF injection from crafted working directories.

### Performance
- Taste content cached against file mtimes instead of two disk reads per
  request.

**Compare**: https://github.com/Lythaeon/cmdcode/compare/v0.3.0...v0.4.0

## [0.3.0] - 2026-08-22

### Added
- **Per-key rate limiting** — configurable request caps per API key with a sliding
  window. Supports in-memory (default) and Redis backends for distributed
  deployments. Set via `COMMAND_CODE_PROXY_MAX_REQUESTS` and
  `COMMAND_CODE_PROXY_RATE_LIMIT_WINDOW`.
- **`cmdcode setup`** — new command to configure client harnesses (OpenCode,
  Codex, Hermes, LiteLLM, Ollama, vLLM, Open WebUI) to route through the
  proxy. Supports `--dry-run` and `--force`.
- **`SensitiveString`** — new zeroizing credential type that auto-redacts on
  drop and in Display. Used for all API keys and tokens.
- **New fuzz targets** — `fuzz_environment`, `fuzz_harness_types`,
  `fuzz_rate_limit_backend`, `fuzz_sensitive_string` added to the CI fuzz
  regression suite.

### Fixed
- **Integration concurrency test** — `test_concurrent_100_requests` now uses a
  shared `reqwest` client instead of spawning 100 separate ones, eliminating
  spurious failures from fd exhaustion and TIME_WAIT slots.
- Minor audit findings and code quality improvements across auth, config,
  and handler modules.

**Compare**: https://github.com/Lythaeon/cmdcode/compare/v0.2.0...v0.3.0

## [0.2.0] - 2026-08-21

CLI subcommand refactor, streaming robustness fixes, and a security-hardening
pass. The most impactful change is a fix for **large opencode sessions
silently returning empty streams** — the proxy now correctly handles the
upstream's multi-megabyte `start-step` event echo and retries genuine empty
streams the same way the official CLI does.

### Highlights
- **CLI subcommands** — replaced the single `cmdcode` launcher with a
  clap-based CLI: `cmdcode serve`, `cmdcode status`, `cmdcode models`,
  `cmdcode config`, `cmdcode auth`, and `cmdcode test` (a full self-diagnostic
  that starts the proxy, sends a real completion, and validates the upstream
  round-trip against your logged-in credentials).
- **Empty-stream retry** — the upstream occasionally accepts a request, emits
  only `{"type":"start"}`, then closes with no content and no `finish` event.
  The proxy now defers the 200 SSE header until the first real chunk, retries
  the upstream call with exponential backoff (mirroring the CLI's
  `callModelWithRetry`), and only surfaces an explicit `502` error after all
  retries are exhausted instead of a silent empty success that made opencode
  sessions appear to "exit for no reason".
- **Oversized-line resilience** — the upstream `start-step` event echoes the
  full request body (tools + messages) on a single NDJSON line, which can
  exceed 1 MiB on large opencode sessions. The proxy previously aborted the
  entire stream, producing `chunks=0` (empty client response). Oversized
  records are now skipped as metadata and the real events after them continue
  to stream. The absolute no-newline DoS cap is retained at 4 MiB.
- **Security hardening pass** (`F-1`–`F-8`):
  - Warn on non-localhost binds without `COMMAND_CODE_PROXY_INCOMING_TOKEN`.
  - Validate CORS `Origin` (no `*` / wildcard / bare scheme bypass).
  - Warn when the upstream URL is plain `http://`.
  - Constant-time token comparison edge-case tests.
- **Expanded test suite** — ~90 new tests across auth, wire format, types,
  plus two new fuzz targets (`fuzz_auth_security`, `fuzz_errors`) and
  security-focused property tests.

### Added
- `cmdcode` CLI subcommands: `serve`, `status`, `models`, `config`, `auth`,
  `test` (clap-based, `cli.rs` + `commands/`).
- `cmdcode_empty_streams_total` Prometheus counter.
- `fuzz_auth_security`, `fuzz_errors` fuzz targets.
- Security-focused proptests (model ID strip, auth/config deserialize,
  error Display) in `proptests.rs`.
- Missing-docs and coverage tests for auth manager, wire-format helpers.

### Fixed
- **Empty upstream stream aborts the whole response** — 1 MiB streaming cap
  tripped on the upstream `start-step` request-echo line, killing large
  sessions with no output. Oversized lines are now skipped; real events after
  them are delivered.
- **Silent empty 200 on upstream close** — a stream that closes with no
  content/no `finish` is now retried with backoff and returns an explicit 502
  after retries, matching the official CLI's `callModelWithRetry` behavior.
- **CLI clippy violations** — the new `test` command had `expect`/`unwrap`
  that failed `-D clippy::all`; replaced with error-propagation + exit codes.

### Security
- **[Low] CORS wildcard reflection** (`F-4`) — `Access-Control-Allow-Origin`
  no longer echoes `*` or bare-scheme origins; only explicit `http(s)://`
  origins are reflected.
- **[Low] CRLF injection via `COMMAND_CODE_ENV`** (`F-8`) — env-derived
  `x-cli-environment` header value sanitized against `\r`/`\n` to prevent
  header injection.
- **[Low] Constant-time comparison edge cases** (`F-3`) — added explicit
  coverage that length-mismatched tokens never compare equal.
- **[Informational] Unauthenticated bind warning** (`F-1`) —
  non-localhost binds without an incoming token warn at startup.
- **[Informational] Plaintext upstream warning** (`F-7`) — `http://`
  upstream URLs warn that credentials are sent in plaintext.

### Dependencies
- No new advisories. Continuing to track `protobuf 2.28.0`
  (RUSTSEC-2024-0437) and `lru 0.16.4` (RUSTSEC-2026-0253) — see v0.1.0.

**Compare**: https://github.com/Lythaeon/cmdcode/compare/v0.1.0...v0.2.0

## [0.1.0] - 2026-08-20

Initial public release. OpenAI-compatible proxy for the Command Code API,
built on Rust/Pingora with sub-millisecond overhead, full SSE streaming
support, and four rounds of adversarial auditing. **No breaking changes**
relative to the pre-release — this is the first tagged version.

### Highlights
- **OpenAI Chat Completions proxy** — transparent `/v1/chat/completions`
  (stream + non-stream) translation to Command Code's NDJSON wire format,
  preserving the exact API fingerprint so upstream cannot distinguish proxy
  traffic from the official `cmd` CLI.
- **Sub-millisecond overhead** — measured warm-loopback p50=+0.38ms,
  p95=+0.52ms, p99=+0.73ms. Proxy overhead is effectively zero relative to
  model inference latency.
- **Concurrency control** — optional semaphore-based concurrency limiter
  (`max_concurrent`) with correct RAII permit lifetime. `0` means unlimited.
- **Stateful cancellation** — `tokio_util::CancellationToken` ensures the
  upstream task is aborted on client disconnect or idle timeout, releasing the
  reqwest connection and semaphore permit immediately.
- **UTF-8 safe streaming** — raw `Vec<u8>` buffer with per-line UTF-8 decode
  prevents multi-byte character corruption across HTTP chunk boundaries.
- **Auth refresh** — 401/403 invalidates cached credentials, re-reads from
  disk, and retries once independently of the network retry budget.
- **Bounded backpressure** — 256-entry mpsc channel between upstream reader
  and handler; 1 MiB maximum unterminated-stream buffer with truncation
  metric.
- **Prometheus metrics** — `cmdcode_*` counters for requests, errors, retries,
  stream truncation, client disconnects, active streams, bytes in/out.
- **Optional TLS and incoming auth** — `COMMAND_CODE_PROXY_TLS_CERT/KEY` for
  TLS termination; `COMMAND_CODE_PROXY_INCOMING_TOKEN` for bearer-token gate.
- **Config validation** — listen host/port validated at startup; model
  allowlist via env var.

### Added
- SSE streaming pipeline with amortized `Vec<u8>` buffer processing and
  residual EOF handling.
- Non-streaming path with `saw_finish` guard — returns 502 if upstream EOF
  arrives without a finish event.
- Concurrency semaphore regression test (`test_semaphore_held_during_stream`)
  that would fail if `let _ = _permit` were reintroduced.
- 10 fuzz targets (`fuzz_wire_format`, `fuzz_upstream_events`, `fuzz_auth`,
  `fuzz_auth_data`, `fuzz_catalog`, `fuzz_model_catalog`, `fuzz_messages`,
  `fuzz_tools`, `fuzz_request_body`, `fuzz_model_effort`) with CI regression.
- Workspace clippy deny lints (`unwrap_used`, `expect_used`, `panic`,
  `todo`, `unimplemented`, `undocumented_unsafe_blocks`) enforced in all
  three crates.
- `cargo install cmdcode` — publishable to crates.io.
- GitHub Actions CI: formatting, clippy (`-D warnings`), workspace tests,
  nightly fuzz regression.
- Release CI: tag-triggered `v*.*.*` workflow with GitHub release creation
  and crates.io publish.
- systemd service unit, soak test script, supervision script.
- Setup docs, OpenCode integration guide.

### Fixed
- **Concurrent semaphore permit leak** (`F-CONC-1`) — streaming tasks
  immediately dropped the semaphore permit via `let _ = _permit;`, bypassing
  the concurrency limit entirely. Fixed with named RAII binding.
- **`active_streams` gauge drift** (`F-CONC-2`) — non-disconnect write errors
  caused early return before `stream_finished()`, permanently incrementing the
  gauge. Fixed with unconditional decrement.
- **`/health` information disclosure** (`F-AUDIT-1`) — endpoint leaked auth
  directory path, credential method, and upstream URL. Fixed to return only
  `status`, `models`, and `default_model`.
- **Unbounded streaming buffer** (`F-AUDIT-2`) — upstream sending data without
  newlines could grow the buffer without bound. Fixed with 1 MiB cap.
- **UTF-8 corruption on chunk-split characters** (`F-NEW-1`) —
  `String::from_utf8_lossy` replaced multi-byte characters split across HTTP
  chunks with U+FFFD. Fixed with raw `Vec<u8)` buffer and per-line decode.
- **Dead `health_check()` method** — exposed auth infrastructure details;
  removed.
- **Dead `AuthMethod::is_valid()`** — never called; removed.
- **Config validation** — malformed `COMMAND_CODE_PROXY_HOST`/`PORT` env vars
  could produce invalid listen addresses. Now validated at startup.
- **Error chain preservation** — `map_err(|_|...)` patterns replaced with
  `map_err(|e|...)` to preserve original error context.
- **Non-disconnect write error cleanup** — early `return Err(e)` replaced with
  `client_gone = true; break` to ensure gauge decrement and cancel signal.

### Security
- **[Low] Timing side-channel on bearer token length** (`F-AUTH-1`) —
  `constant_time_eq` returns immediately on length mismatch, leaking token
  length via response timing. Accepted: inherent to fixed-length bearer token
  checks; mitigated by using high-entropy random tokens.
- **[Informational] Unauthenticated model catalog** (`CC-PROXY-004`) —
  `/v1/models` bypasses the auth gate. Accepted: standard for OpenAI-compatible
  APIs.
- **[Informational] Workspace file listing sent to upstream** (`CC-PROXY-006`)
  — `build_config()` sends the working directory structure. Accepted: required
  for upstream workspace context.

### Dependencies
- **[Tracked] protobuf 2.28.0** (`CC-PROXY-002`, RUSTSEC-2024-0437) —
  transitive via `prometheus 0.13.4`. No patched 2.x version available;
  unreachable from untrusted input. Tracking upstream prometheus migration.
- **[Tracked] lru 0.16.4** (`CC-PROXY-003`, RUSTSEC-2026-0253) — transitive
  via `pingora-cache`/`pingora-pool`. Unsound `LruCache::pop()` not
  exploitable with string keys. Tracking upstream pingora update.

**Compare**: https://github.com/Lythaeon/cmdcode/compare/initial...v0.1.0
