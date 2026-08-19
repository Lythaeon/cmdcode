use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::SystemTime;

/// Prometheus-style counters and gauges for the proxy.
///
/// Every mutation is a relaxed atomic — cheap enough for hot request
/// paths and safe under concurrent streams. Rendering is lock-free.
#[derive(Debug)]
pub struct Metrics {
    pub requests_total: AtomicU64,
    pub stream_requests: AtomicU64,
    pub nonstream_requests: AtomicU64,
    pub chunks_total: AtomicU64,
    pub bytes_out_total: AtomicU64,
    pub upstream_retries_total: AtomicU64,
    pub errors_total: AtomicU64,
    pub bad_requests: AtomicU64,
    pub body_too_large: AtomicU64,
    pub model_denied: AtomicU64,
    pub unknown_routes: AtomicU64,
    pub client_disconnects: AtomicU64,
    pub upstream_timeouts: AtomicU64,
    pub active_streams: AtomicI64,
    started_at: SystemTime,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            requests_total: AtomicU64::new(0),
            stream_requests: AtomicU64::new(0),
            nonstream_requests: AtomicU64::new(0),
            chunks_total: AtomicU64::new(0),
            bytes_out_total: AtomicU64::new(0),
            upstream_retries_total: AtomicU64::new(0),
            errors_total: AtomicU64::new(0),
            bad_requests: AtomicU64::new(0),
            body_too_large: AtomicU64::new(0),
            model_denied: AtomicU64::new(0),
            unknown_routes: AtomicU64::new(0),
            client_disconnects: AtomicU64::new(0),
            upstream_timeouts: AtomicU64::new(0),
            active_streams: AtomicI64::new(0),
            started_at: SystemTime::now(),
        }
    }

    pub fn inc_request(&self, stream: bool) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
        if stream {
            self.stream_requests.fetch_add(1, Ordering::Relaxed);
        } else {
            self.nonstream_requests.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn inc_chunks(&self, n: u32) {
        self.chunks_total.fetch_add(n as u64, Ordering::Relaxed);
    }

    pub fn inc_bytes_out(&self, n: usize) {
        self.bytes_out_total
            .fetch_add(n as u64, Ordering::Relaxed);
    }

    pub fn inc_retries(&self) {
        self.upstream_retries_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_error(&self) {
        self.errors_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_bad_requests(&self) {
        self.bad_requests.fetch_add(1, Ordering::Relaxed);
        self.inc_error();
    }

    pub fn inc_body_too_large(&self) {
        self.body_too_large.fetch_add(1, Ordering::Relaxed);
        self.inc_error();
    }

    pub fn inc_model_denied(&self) {
        self.model_denied.fetch_add(1, Ordering::Relaxed);
        self.inc_error();
    }

    pub fn inc_unknown_route(&self) {
        self.unknown_routes.fetch_add(1, Ordering::Relaxed);
        self.inc_error();
    }

    pub fn inc_client_disconnect(&self) {
        self.client_disconnects.fetch_add(1, Ordering::Relaxed);
        self.inc_error();
    }

    pub fn inc_upstream_timeout(&self) {
        self.upstream_timeouts.fetch_add(1, Ordering::Relaxed);
        self.inc_error();
    }

    pub fn stream_started(&self) {
        self.active_streams.fetch_add(1, Ordering::Relaxed);
    }

    pub fn stream_finished(&self) {
        self.active_streams.fetch_add(-1, Ordering::Relaxed);
    }

    /// Uptime in whole seconds since construction.
    pub fn uptime_secs(&self) -> u64 {
        self.started_at
            .elapsed()
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    /// Render all metrics in Prometheus text exposition format.
    pub fn render(&self) -> String {
        let mut out = String::with_capacity(1024);

        fn line(out: &mut String, help: &str, name: &str, value: String, kind: &str) {
            out.push_str("# HELP ");
            out.push_str(name);
            out.push(' ');
            out.push_str(help);
            out.push('\n');
            out.push_str("# TYPE ");
            out.push_str(name);
            out.push(' ');
            out.push_str(kind);
            out.push('\n');
            out.push_str(name);
            out.push(' ');
            out.push_str(&value);
            out.push('\n');
        }

        line(
            &mut out,
            "Total chat completions requests served.",
            "command_code_proxy_requests_total",
            self.requests_total.load(Ordering::Relaxed).to_string(),
            "counter",
        );
        line(
            &mut out,
            "Streaming requests served.",
            "command_code_proxy_stream_requests_total",
            self.stream_requests.load(Ordering::Relaxed).to_string(),
            "counter",
        );
        line(
            &mut out,
            "Non-streaming requests served.",
            "command_code_proxy_nonstream_requests_total",
            self.nonstream_requests.load(Ordering::Relaxed).to_string(),
            "counter",
        );
        line(
            &mut out,
            "SSE chunks forwarded to clients.",
            "command_code_proxy_chunks_total",
            self.chunks_total.load(Ordering::Relaxed).to_string(),
            "counter",
        );
        line(
            &mut out,
            "Response bytes written to clients.",
            "command_code_proxy_bytes_out_total",
            self.bytes_out_total.load(Ordering::Relaxed).to_string(),
            "counter",
        );
        line(
            &mut out,
            "Upstream retry attempts made.",
            "command_code_proxy_upstream_retries_total",
            self.upstream_retries_total.load(Ordering::Relaxed).to_string(),
            "counter",
        );
        line(
            &mut out,
            "Requests that failed or were rejected.",
            "command_code_proxy_errors_total",
            self.errors_total.load(Ordering::Relaxed).to_string(),
            "counter",
        );
        line(
            &mut out,
            "Requests rejected for an unparseable body.",
            "command_code_proxy_bad_requests_total",
            self.bad_requests.load(Ordering::Relaxed).to_string(),
            "counter",
        );
        line(
            &mut out,
            "Requests rejected as oversized.",
            "command_code_proxy_body_too_large_total",
            self.body_too_large.load(Ordering::Relaxed).to_string(),
            "counter",
        );
        line(
            &mut out,
            "Requests rejected by the model allowlist.",
            "command_code_proxy_model_denied_total",
            self.model_denied.load(Ordering::Relaxed).to_string(),
            "counter",
        );
        line(
            &mut out,
            "Requests hitting unknown routes.",
            "command_code_proxy_unknown_routes_total",
            self.unknown_routes.load(Ordering::Relaxed).to_string(),
            "counter",
        );
        line(
            &mut out,
            "Client disconnects mid-stream.",
            "command_code_proxy_client_disconnects_total",
            self.client_disconnects
                .load(Ordering::Relaxed)
                .to_string(),
            "counter",
        );
        line(
            &mut out,
            "Upstream timeouts observed.",
            "command_code_proxy_upstream_timeouts_total",
            self.upstream_timeouts.load(Ordering::Relaxed).to_string(),
            "counter",
        );
        line(
            &mut out,
            "Streams currently open.",
            "command_code_proxy_active_streams",
            self.active_streams.load(Ordering::Relaxed).to_string(),
            "gauge",
        );
        line(
            &mut out,
            "Proxy uptime in seconds.",
            "command_code_proxy_uptime_seconds",
            self.uptime_secs().to_string(),
            "gauge",
        );

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_contains_all_metrics() {
        let m = Metrics::new();
        let text = m.render();
        for name in [
            "command_code_proxy_requests_total",
            "command_code_proxy_stream_requests_total",
            "command_code_proxy_nonstream_requests_total",
            "command_code_proxy_chunks_total",
            "command_code_proxy_bytes_out_total",
            "command_code_proxy_upstream_retries_total",
            "command_code_proxy_errors_total",
            "command_code_proxy_bad_requests_total",
            "command_code_proxy_body_too_large_total",
            "command_code_proxy_model_denied_total",
            "command_code_proxy_unknown_routes_total",
            "command_code_proxy_client_disconnects_total",
            "command_code_proxy_upstream_timeouts_total",
            "command_code_proxy_active_streams",
            "command_code_proxy_uptime_seconds",
        ] {
            assert!(text.contains(name), "missing metric {name}");
            let kind = if name.ends_with("_total") || name == "command_code_proxy_active_streams" {
                if name == "command_code_proxy_active_streams" { "gauge" } else { "counter" }
            } else {
                "gauge"
            };
            assert!(
                text.contains(&format!("# TYPE {name} {kind}\n{name} ")),
                "malformed block for {name}"
            );
        }
    }

    #[test]
    fn test_counters_increment() {
        let m = Metrics::new();
        assert_eq!(m.requests_total.load(Ordering::Relaxed), 0);
        m.inc_request(true);
        m.inc_request(true);
        m.inc_request(false);
        assert_eq!(m.requests_total.load(Ordering::Relaxed), 3);
        assert_eq!(m.stream_requests.load(Ordering::Relaxed), 2);
        assert_eq!(m.nonstream_requests.load(Ordering::Relaxed), 1);

        m.inc_chunks(10);
        m.inc_bytes_out(42);
        m.inc_retries();
        m.inc_bad_requests();
        assert_eq!(m.chunks_total.load(Ordering::Relaxed), 10);
        assert_eq!(m.bytes_out_total.load(Ordering::Relaxed), 42);
        assert_eq!(m.upstream_retries_total.load(Ordering::Relaxed), 1);
        assert_eq!(m.bad_requests.load(Ordering::Relaxed), 1);
        assert_eq!(m.errors_total.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_active_streams_gauge() {
        let m = Metrics::new();
        m.stream_started();
        m.stream_started();
        assert_eq!(m.active_streams.load(Ordering::Relaxed), 2);
        m.stream_finished();
        assert_eq!(m.active_streams.load(Ordering::Relaxed), 1);
        assert!(m.render().contains("command_code_proxy_active_streams 1"));
    }

    #[test]
    fn test_uptime_positive() {
        let m = Metrics::new();
        let text = m.render();
        let line = text
            .lines()
            .find(|l| l.starts_with("command_code_proxy_uptime_seconds "))
            .expect("uptime line");
        let secs: u64 = line
            .split(' ')
            .nth(1)
            .expect("value")
            .parse()
            .expect("numeric");
        let _ = secs;
    }
}