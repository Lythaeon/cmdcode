# Setup Guide

## Prerequisites

- Rust 1.75+ (with cargo)
- `command-code` CLI installed and authenticated (`command-code login`)
- `~/.commandcode/auth.json` exists with a valid API key

## Quick start

```bash
# Clone the repo
git clone https://github.com/Lythaeon/command-code-openapi-proxy.git
cd command-code-openapi-proxy

# Build and run
cargo run --release

# Or build first, then run
cargo build --release
./target/release/command-code-proxy
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
| `COMMAND_CODE_LOCAL` | (unset) | Set `1` to use `localhost:9090` |

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
cd /path/to/command-code-openapi-proxy
cargo build --release
```

### Step 2: Install the binary

```bash
# Create ~/.local/bin if it doesn't exist
mkdir -p ~/.local/bin

# Copy the binary
cp target/release/command-code-proxy ~/.local/bin/

# Make sure ~/.local/bin is in your PATH
# Add to ~/.bashrc or ~/.zshrc if not already:
#   export PATH="$HOME/.local/bin:$PATH"

# Verify it works
~/.local/bin/command-code-proxy --help
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
docker build -t command-code-proxy .

# Run
docker run -d \
  --name command-code-proxy \
  -p 18080:18080 \
  -v ~/.commandcode:/root/.commandcode:ro \
  -e COMMAND_CODE_PROXY_HOST=0.0.0.0 \
  command-code-proxy
```

## Run behind nginx (production)

```nginx
upstream command_code_proxy {
    server 127.0.0.1:18080;
}

server {
    listen 443 ssl;
    server_name proxy.example.com;

    location /v1/ {
        proxy_pass http://command_code_proxy;
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
