#!/usr/bin/env bash
#
# Restart-on-crash supervisor for command-code-proxy.
#
# Runs the proxy in a loop: if it dies with a non-zero exit code it is
# restarted with exponential backoff (1s -> 30s cap). A clean exit (0) is
# treated as an intentional stop and ends the supervisor. SIGTERM/SIGINT
# forward to the child and stop cleanly.
#
# Env overrides:
#   COMMAND_CODE_PROXY_BIN                proxy binary (default ~/.local/bin/command-code-proxy)
#   COMMAND_CODE_PROXY_SUPERVISOR_LOG     supervisor+proxy log (default $BIN-supervisor.log)
#   COMMAND_CODE_PROXY_PIDFILE            child pid file (default $BIN.pid)
#   COMMAND_CODE_PROXY_LOCKFILE           single-instance lock (default /tmp/command-code-proxy-supervisor.lock)
#
set -u

BIN="${COMMAND_CODE_PROXY_BIN:-$HOME/.local/bin/command-code-proxy}"
SUPLOG="${COMMAND_CODE_PROXY_SUPERVISOR_LOG:-${BIN}-supervisor.log}"
PIDFILE="${COMMAND_CODE_PROXY_PIDFILE:-${BIN}.pid}"
LOCKFILE="${COMMAND_CODE_PROXY_LOCKFILE:-/tmp/command-code-proxy-supervisor.lock}"

# Give the child a self-rotating log by default (overridable).
export COMMAND_CODE_PROXY_LOG_FILE="${COMMAND_CODE_PROXY_LOG_FILE:-${BIN}-rotated.log}"

# Single-instance guard.
exec 9>"$LOCKFILE"
if ! flock -n 9; then
    echo "supervisor already running (lock $LOCKFILE)" >&2
    exit 1
fi

log() { printf '%s %s\n' "$(date -Is)" "$*" >>"$SUPLOG"; }

stop() {
    trap - TERM INT
    log "supervisor stopping (signal)"
    if [ -f "$PIDFILE" ]; then
        kill "$(cat "$PIDFILE")" 2>/dev/null || true
        rm -f "$PIDFILE"
    fi
    exit 0
}
trap stop TERM INT

[ -x "$BIN" ] || { log "binary not executable: $BIN"; exit 1; }

backoff=1
while :; do
    log "starting $BIN"
    "$BIN" >>"$SUPLOG" 2>&1 &
    child=$!
    echo "$child" >"$PIDFILE"
    wait "$child"
    rc=$?
    rm -f "$PIDFILE"
    if [ "$rc" -eq 0 ]; then
        log "exited cleanly (rc=0); supervisor stopping"
        exit 0
    fi
    log "proxy crashed (rc=$rc); restarting in ${backoff}s"
    sleep "$backoff"
    [ "$backoff" -lt 30 ] && backoff=$((backoff * 2))
done