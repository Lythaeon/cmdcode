# Setup Guide

## Prerequisites

- Python 3.8+
- `command-code` CLI installed and authenticated (`command-code login`)
- `~/.commandcode/auth.json` exists with a valid API key

## Quick start

```bash
# Clone the repo
git clone https://github.com/Lythaeon/command-code-openapi-proxy.git
cd command-code-openapi-proxy

# Run directly
python3 -m command_code_proxy.proxy

# Or install as a package
pip install .
command-code-proxy
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
    "model": "gpt-5.6-luna",
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
| `COMMAND_CODE_LOCAL` | (unset) | Set `1` to use `localhost:9090` |
| `COMMANDCODE_SKIP_UPDATES` | (unset) | Set `1` to disable auto-updates |

## Run as systemd service (Linux)

```bash
# Install the service file
cp systemd/command-code-proxy.service ~/.config/systemd/user/

# Reload and start
systemctl --user daemon-reload
systemctl --user enable --now command-code-proxy

# Check logs
journalctl --user -u command-code-proxy -f
```

## Run with Docker

```bash
docker run -d \
  --name command-code-proxy \
  -p 18080:18080 \
  -v ~/.commandcode:/root/.commandcode:ro \
  -e COMMAND_CODE_PROXY_HOST=0.0.0.0 \
  python:3.12-slim \
  bash -c "pip install /path/to/repo && command-code-proxy"
```

Or build from the Dockerfile:

```bash
docker build -t command-code-proxy .
docker run -d \
  --name command-code-proxy \
  -p 18080:18080 \
  -v ~/.commandcode:/root/.commandcode:ro \
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
