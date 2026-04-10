# Copilot instructions for this repository

## Build, test, and lint commands

Use Cargo for all build and test workflows:

```bash
cargo build                 # Debug build
cargo build --release       # Optimized release build
cargo run                   # Run the local proxy server

cargo test                  # All unit and integration tests
cargo test --lib            # Unit tests only
cargo test --test api_test  # Integration tests only
cargo test <test_name>      # Run a single test by name

cargo check                 # Fast type/syntax check
cargo clippy                # Linting
```

Single-test examples from the current tree:

```bash
cargo test defaults_when_env_empty
cargo test chat_request_roundtrip
cargo test cached_copilot_token_returned_when_valid
```

On macOS, `daemon.sh` wraps the release binary for launchd usage:

```bash
./daemon.sh build
./daemon.sh install
./daemon.sh start
./daemon.sh stop
./daemon.sh status
./daemon.sh logs
```

## High-level architecture

This project is a Rust `axum` reverse proxy that exposes OpenAI-compatible endpoints locally and forwards them to the GitHub Copilot Chat API.

Request flow:

1. `src/main.rs` loads settings, initializes tracing, creates `CopilotAuth` and `CopilotClient`, then starts the Axum server.
2. `src/lib.rs` builds the shared `AppState` and attaches permissive CORS middleware.
3. `src/routes.rs` exposes `GET /health`, `GET /v1/models`, and `POST /v1/chat/completions`.
4. `src/routes.rs` checks the incoming `stream` flag and dispatches to non-streaming JSON or streaming SSE handling.
5. `src/copilot.rs` builds the upstream GitHub Copilot request, including the VS Code/Copilot-style headers expected by the API.
6. `src/auth.rs` provides GitHub authentication, exchanging a GitHub token for a Copilot token and refreshing it before expiry.
7. The upstream response is returned either as OpenAI-style JSON or as SSE `data:` frames.

Module responsibilities:

- `src/config.rs`: best-effort `.env` loading, then environment-variable-based settings with defaults.
- `src/auth.rs`: GitHub OAuth Device Flow, persisted GitHub token loading, Copilot token caching, refresh, and re-auth on stale credentials.
- `src/copilot.rs`: upstream HTTP client, model listing, non-streaming requests, and byte-stream-to-SSE conversion.
- `src/models.rs`: OpenAI-compatible request/response/error structs used by the proxy surface.
- `src/routes.rs`: endpoint wiring and HTTP status/error mapping.
- `tests/api_test.rs`: integration tests that boot the Axum app against `wiremock` instead of real GitHub services.

## Key conventions

- The current implementation is Rust, not Python. Trust `Cargo.toml`, `src/*.rs`, and `tests/*.rs` over older README sections that still describe a FastAPI layout.
- `.env` loading is manual and best-effort in `src/config.rs`; existing environment variables win over values from `.env`.
- Default runtime settings are `HOST=127.0.0.1`, `PORT=8080`, and `LOG_LEVEL=info`. `GITHUB_TOKEN` is optional and skips Device Flow when present.
- GitHub tokens are cached on disk at `$XDG_CONFIG_HOME/copilot-wrapper/github_token` or `~/.config/copilot-wrapper/github_token`.
- `CopilotAuth` refreshes Copilot tokens with a five-minute safety margin instead of waiting for hard expiry.
- Header spoofing is intentional. `src/auth.rs` and `src/copilot.rs` send VS Code / Copilot-style headers; preserve that behavior when changing upstream calls.
- `GET /v1/models` tolerates upstream failures by returning a hardcoded fallback model list from `src/copilot.rs`.
- Streaming responses are true SSE. Keep `text/event-stream` headers and `data: ...\n\n` framing aligned with the logic in `src/routes.rs` and `src/copilot.rs`.
- Route error mapping is deliberate: auth-related failures become `401`, while upstream failures are surfaced as `502`.
- Tests are split by scope: small serialization/config/auth checks live inline under `#[cfg(test)]`, while end-to-end HTTP behavior lives in `tests/api_test.rs`.
- Integration tests should keep using `CopilotClient::new_with_base_urls(...)` plus `wiremock` so they never depend on real credentials or the live Copilot API.
