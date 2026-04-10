# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Copilot Gateway** is a local reverse proxy that exposes OpenAI-compatible API endpoints (`/v1/models`, `/v1/chat/completions`) and forwards requests to the GitHub Copilot Chat API. It handles GitHub OAuth Device Flow authentication on first run, then auto-refreshes Copilot tokens as needed.

## Commands

```bash
# Build
cargo build --release       # Optimized binary (~2.4 MB stripped)
cargo build                 # Debug build

# Run
cargo run                   # Run from source (uses .env if present)

# Test
cargo test                  # All tests (unit + integration)
cargo test --lib            # Unit tests only
cargo test --test api_test  # Integration tests only
cargo test <name>           # Single test by name

# Lint/Check
cargo check                 # Fast syntax/type check
cargo clippy                # Linting

# Daemon (macOS)
./daemon.sh build           # Build release binary
./daemon.sh install         # Install as launchd daemon (auto-start on login)
./daemon.sh uninstall       # Remove daemon
./daemon.sh start|stop      # Control daemon
./daemon.sh logs            # Tail daemon logs
```

## Architecture

```
OpenAI Client → localhost:8080 → GitHub Copilot API
                                      ↕
                            GitHub OAuth (first run)
```

### Module Responsibilities

- **`config.rs`** — Loads `.env` then env vars. Settings: `HOST`, `PORT`, `LOG_LEVEL`, optional `GITHUB_TOKEN`.
- **`auth.rs`** — `CopilotAuth`: GitHub OAuth Device Flow (saves token to `~/.config/copilot-wrapper/github_token`), exchanges GitHub token for Copilot API token, auto-refreshes with 5-min expiry margin. VS Code client headers are hardcoded to mimic the official extension.
- **`copilot.rs`** — `CopilotClient`: async HTTP client wrapping the Copilot API. Handles non-streaming JSON responses and SSE streaming. Falls back to a hardcoded model list if the models endpoint is unavailable. Timeouts: 120s request, 10s connect.
- **`models.rs`** — Serde structs for OpenAI-compatible request/response format. Contains unit tests for serialization roundtrips.
- **`routes.rs`** — Axum handlers for `GET /health`, `GET /v1/models`, `POST /v1/chat/completions`. Detects `stream` flag, maps 401/auth errors → `UNAUTHORIZED`, other errors → `BAD_GATEWAY`.
- **`lib.rs`** — Defines `AppState` (holds `CopilotClient`) and `build_app()` which wires CORS middleware and routes.
- **`main.rs`** — Entry point: loads config, initializes tracing, creates auth + client, starts TCP listener with graceful shutdown.

### Request Flow

1. Client POSTs OpenAI-format JSON to `/v1/chat/completions`
2. Handler checks `stream` flag
3. `CopilotClient` calls `CopilotAuth::get_copilot_token()` (cached, refreshed as needed)
4. Request forwarded to `api.githubcopilot.com` with Bearer token
5. Response returned as JSON or SSE stream

### Integration Tests

Tests in `tests/api_test.rs` use `wiremock` to mock the upstream Copilot API. `test_app()` wires the mock server URL as the Copilot base URL so no real credentials are needed.

## Configuration

Copy `.env.example` to `.env` to configure locally. All variables have defaults and are optional except `GITHUB_TOKEN` (if skipping OAuth flow):

| Variable | Default | Description |
|---|---|---|
| `HOST` | `127.0.0.1` | Bind address |
| `PORT` | `8080` | Listen port |
| `LOG_LEVEL` | `info` | Tracing level |
| `GITHUB_TOKEN` | — | Skip OAuth; use this token directly |
