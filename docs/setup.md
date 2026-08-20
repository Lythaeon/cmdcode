# Setup Guide

## Prerequisites

- Rust 1.75+ (with cargo)
- `command-code` CLI installed and authenticated (`command-code login`)
- `~/.commandcode/auth.json` exists with a valid API key

## Quick start

```bash
# Clone the repo
git clone https://github.com/Lythaeon/cmdcode.git
cd cmdcode

# Build and run
cargo run --release

# Or build first, then run
cargo build --release
./target/release/cmdcode
```

The proxy starts on `http://127.0.0.1:18080`.

## Verify it works

```bash
# Health check
curl http://127.0.0.1:18080/health

# List models
curl http://127.0.0.1:18080/v1/models

# Send a completion
curl http://127.0.0.1:18080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "xiaomi/mimo-v2.5",
    "messages": [{"role": "user", "content": "Say hello"}],
    "max_tokens": 100
  }'
```

## Configuration

Environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `COMMAND_CODE_PROXY_PORT` | `18080` | Listen port |
| `COMMAND_CODE_PROXY_HOST` | `127.0.0.1` | Bind address |
| `COMMAND_CODE_API_BASE` | `https://api.commandcode.ai` | Upstream API |
| `COMMAND_CODE_PROXY_TIMEOUT` | `600` | Upstream timeout (seconds) |
| `COMMAND_CODE_PROXY_RETRIES` | `2` | Retry count for 502/503/504 |
| `COMMAND_CODE_PROXY_CORS` | (unset) | CORS origin header |
| `COMMAND_CODE_PROXY_DEFAULT` | `xiaomi/mimo-v2.5` | Default model |
| `COMMAND_CODE_PROXY_MODELS` | (unset) | Comma-separated model allowlist |
| `COMMAND_CODE_PROXY_MAX_REQS` | `0` | Max concurrent requests (0=unlimited) |
| `COMMAND_CODE_PROXY_LOG_FILE` | (unset) | Write logs to this file with size-based rotation |
| `COMMAND_CODE_PROXY_LOG_MAX_BYTES` | `52428800` | Rotate the log after this many bytes (50MB) |
| `COMMAND_CODE_PROXY_LOG_KEEP` | `5` | Keep this many rotated log backups |
| `COMMAND_CODE_PROXY_INCOMING_TOKEN` | (unset) | Require `Authorization: Bearer <token>` on API routes |
| `COMMAND_CODE_PROXY_TLS_CERT` | (unset) | Path to a TLS certificate (enables HTTPS; needs KEY too) |
| `COMMAND_CODE_PROXY_TLS_KEY` | (unset) | Path to the matching TLS private key |
| `COMMAND_CODE_LOCAL` | (unset) | Set `1` to use `localhost:9090` |

`/metrics` and `/health` are always served unauthenticated for monitors and
scrapers. When `COMMAND_CODE_PROXY_INCOMING_TOKEN` is set, every other route
(chat completions, models) requires that token as a bearer header.

## Supervision, log rotation, and soak

The `scripts/` directory ships three helpers:

## Supervision and log rotation

**systemd is the preferred supervision mechanism.** A user unit ships at
`systemd/command-code-proxy.service` with crash-restart (`Restart=on-failure`),
a stop timeout (`TimeoutStopSec=10`) so restarts do not block on the proxy's
graceful drain, and a restart-loop guard. Install it with:

```bash
cp systemd/command-code-proxy.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now command-code-proxy
systemctl --user status command-code-proxy     # or journalctl --user -u command-code-proxy
```

The unit sets `COMMAND_CODE_PROXY_LOG_FILE` so the proxy writes its own
size-rotated log regardless of journald state.

`scripts/` also ships:

- `scripts/supervise.sh` — a fallback for machines without working systemd
  user services. Restarts the proxy on crash with exponential backoff (1s to
  30s cap). A clean exit (0) stops it; SIGTERM/SIGINT forward to the child.
- `scripts/install-supervisor.sh` — installs a `@reboot` cron entry for the
  supervisor and starts it now.
- `scripts/soak.sh` — long-duration memory/health soak. Every
  `SOAK_SAMPLE_SECS` (default 300) it checks `/health`, issues one streaming
  chat request, and samples the proxy's RSS, failing if RSS grows more than
  `SOAK_RSS_GROWTH_MB` (default 200MB) above baseline or a request fails.
  Configure with `SOAK_HOURS` (default 24).

```bash
SOAK_HOURS=8 scripts/soak.sh         # run an 8-hour soak
```

For TLS, set `COMMAND_CODE_PROXY_TLS_CERT` and `COMMAND_CODE_PROXY_TLS_KEY`
to a PEM certificate and key pair. Both must be set together; the proxy then
serves HTTPS on the configured listen address.

## Run with makefile.toml

```bash
# Install cargo-make
cargo install cargo-make

# Available tasks
cargo make build          # Build debug
cargo make build-release  # Build release
cargo make test           # Run all tests
cargo make clippy         # Lint with deny-level warnings
cargo make fmt            # Format code
cargo make ci             # fmt-check + clippy + test
```

## Run as systemd service (Linux)

This sets up the proxy to start automatically on boot and restart on failure.

### Step 1: Build the release binary

```bash
cd /path/to/cmdcode
cargo build --release
```

### Step 2: Install the binary

```bash
# Create ~/.local/bin if it doesn't exist
mkdir -p ~/.local/bin

# Copy the binary
cp target/release/cmdcode ~/.local/bin/

# Make sure ~/.local/bin is in your PATH
# Add to ~/.bashrc or ~/.zshrc if not already:
#   export PATH="$HOME/.local/bin:$PATH"

# Verify it works
~/.local/bin/cmdcode --help
```

### Step 3: Install the systemd service

```bash
# Create the systemd user directory
mkdir -p ~/.config/systemd/user

# Copy the service file
cp systemd/command-code-proxy.service ~/.config/systemd/user/

# Edit the service file to set your username
# Replace YOUR_USERNAME with your actual username:
sed -i "s/YOUR_USERNAME/$(whoami)/" ~/.config/systemd/user/command-code-proxy.service
```

### Step 4: Enable and start the service

```bash
# Reload systemd to pick up the new service
systemctl --user daemon-reload

# Enable the service (starts on boot)
systemctl --user enable command-code-proxy

# Start the service now
systemctl --user start command-code-proxy

# Verify it's running
systemctl --user status command-code-proxy
```

### Step 5: Check logs

```bash
# Follow the logs
journalctl --user -u command-code-proxy -f

# Or check recent logs
journalctl --user -u command-code-proxy --since "1 hour ago"
```

### Step 6: Manage the service

```bash
# Stop the service
systemctl --user stop command-code-proxy

# Restart after config changes
systemctl --user restart command-code-proxy

# Disable from starting on boot
systemctl --user disable command-code-proxy

# Check if it's running
systemctl --user is-active command-code-proxy
```

### Override environment variables

To change the proxy's configuration without editing the service file:

```bash
# Create an override directory
mkdir -p ~/.config/systemd/user/command-code-proxy.service.d

# Create an override file
cat > ~/.config/systemd/user/command-code-proxy.service.d/override.conf << EOF
[Service]
Environment=COMMAND_CODE_PROXY_PORT=18081
Environment=COMMAND_CODE_API_BASE=https://staging.api.commandcode.ai
Environment=RUST_LOG=debug
EOF

# Reload and restart
systemctl --user daemon-reload
systemctl --user restart command-code-proxy
```

## Run with Docker

```bash
# Build the image
docker build -t cmdcode .

# Run
docker run -d \
  --name cmdcode \
  -p 18080:18080 \
  -v ~/.commandcode:/root/.commandcode:ro \
  -e COMMAND_CODE_PROXY_HOST=0.0.0.0 \
  cmdcode
```

## Run behind nginx (production)

```nginx
upstream cmdcode_backend {
    server 127.0.0.1:18080;
}

server {
    listen 443 ssl;
    server_name proxy.example.com;

    location /v1/ {
        proxy_pass http://cmdcode_backend;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_buffering off;
        proxy_cache off;
        proxy_read_timeout 600s;
    }
}
```

## Architecture

See [architecture.md](architecture.md) for wire format translation details.
