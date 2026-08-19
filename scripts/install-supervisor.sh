#!/usr/bin/env bash
#
# Install the proxy supervisor to start on boot (@reboot) and start it now.
#
# Adds (or replaces) a @reboot cron entry that runs supervise.sh, then
# launches the supervisor detached. Safe to re-run: the cron line for this
# supervisor is replaced, and an existing supervisor is left alone.
#
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
SUPERVISOR="$HERE/supervise.sh"

chmod +x "$SUPERVISOR" "$HERE/soak.sh"

# Install/replace the @reboot cron entry.
crontab -l 2>/dev/null | grep -vF "$SUPERVISOR" > /tmp/cc-proxy-cron.$$ || true
printf '@reboot %s\n' "$SUPERVISOR" >> /tmp/cc-proxy-cron.$$
crontab /tmp/cc-proxy-cron.$$
rm -f /tmp/cc-proxy-cron.$$

# Start now, unless already supervised.
if pgrep -f "[s]upervise.sh" >/dev/null 2>&1; then
    echo "supervisor already running"
    exit 0
fi
setsid "$SUPERVISOR" >/dev/null 2>&1 < /dev/null &
echo "supervisor started (pid $!) and installed for @reboot"

# Give it a moment and confirm the proxy comes up.
sleep 2
if pgrep -f "[c]ommand-code-proxy$" >/dev/null 2>&1; then
    echo "proxy is running"
else
    echo "WARNING: proxy did not start; check the supervisor log"
fi