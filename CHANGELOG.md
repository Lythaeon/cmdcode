# Changelog

All notable changes to cmdcode are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/), and this project adheres to [Semantic Versioning](https://semver.org/).

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
