#!/usr/bin/env bash
#
# Long-duration memory/health soak for cmdcode.
#
# Every SOAK_SAMPLE_SECS (default 300) it checks /health, issues one small
# streaming chat request, and samples the proxy's RSS. Reports peak RSS and
# fails if RSS grows past SOAK_RSS_GROWTH_MB above the first sample, or if a
# health check / request fails. Writes a timestamped log to $SOAK_OUT.
#
# Env overrides:
#   SOAK_HOURS          duration (default 24)
#   SOAK_SAMPLE_SECS    sampling interval (default 300)
#   SOAK_RSS_GROWTH_MB  allowed RSS growth over baseline (default 200)
#   SOAK_OUT            output log (default $BIN-soak.log)
#   COMMAND_CODE_PROXY_BASE     proxy base URL (default http://127.0.0.1:18080)
#   COMMAND_CODE_PROXY_BIN      proxy binary, to locate the process (default ~/.local/bin/cmdcode)
#   COMMAND_CODE_PROXY_INCOMING_TOKEN  token if the proxy enforces incoming auth
#
set -u

HOURS="${SOAK_HOURS:-24}"
SAMPLE_SECS="${SOAK_SAMPLE_SECS:-300}"
GROWTH_MB="${SOAK_RSS_GROWTH_MB:-200}"
BASE="${COMMAND_CODE_PROXY_BASE:-http://127.0.0.1:18080}"
BIN="${COMMAND_CODE_PROXY_BIN:-$HOME/.local/bin/cmdcode}"
OUT="${SOAK_OUT:-${BIN}-soak.log}"
TOKEN="${COMMAND_CODE_PROXY_INCOMING_TOKEN:-}"

log() { printf '%s %s\n' "$(date -Is)" "$*" >>"$OUT"; }

rss_of() {
    # RSS of the newest matching process (kB).
    local pid
    pid=$(pgrep -f "[c]mdcode$" | head -n1)
    [ -n "$pid" ] || { echo 0; return; }
    awk '/^VmRSS:/ {print $2}' "/proc/$pid/status" 2>/dev/null || echo 0
}

baseline=""
peak=0
deadline=$(( $(date +%s) + HOURS * 3600 ))

log "soak start: ${HOURS}h, sample ${SAMPLE_SECS}s, rss growth limit ${GROWTH_MB}MB"

while [ "$(date +%s)" -lt "$deadline" ]; do
    # Health check.
    health=$(curl -s -m 10 -o /dev/null -w '%{http_code}' "$BASE/health")
    if [ "$health" != "200" ]; then
        log "FAIL: health check returned $health"
        exit 1
    fi

    # One small streaming request.
    auth=()
    [ -n "$TOKEN" ] && auth=(-H "Authorization: Bearer $TOKEN")
    code=$(curl -s -m 60 -o /dev/null -w '%{http_code}' \
        -H 'content-type: application/json' \
        "${auth[@]}" \
        -X POST "$BASE/v1/chat/completions" \
        -d '{"model":"xiaomi/mimo-v2.5","stream":true,"messages":[{"role":"user","content":"soak ping"}]}')
    if [ "$code" != "200" ]; then
        log "FAIL: chat request returned $code"
        exit 1
    fi

    rss=$(rss_of)
    [ "$rss" -gt "$peak" ] && peak=$rss
    if [ -z "$baseline" ]; then baseline=$rss; fi

    growth_mb=$(( (peak - baseline) / 1024 ))
    log "health=200 rss=${rss}kB peak=${peak}kB baseline=${baseline}kB growth=${growth_mb}MB"

    if [ "$growth_mb" -gt "$GROWTH_MB" ]; then
        log "FAIL: RSS growth ${growth_mb}MB exceeds limit ${GROWTH_MB}MB"
        exit 1
    fi

    sleep "$SAMPLE_SECS"
done

log "soak done: peak RSS ${peak}kB, baseline ${baseline}kB, OK"
exit 0