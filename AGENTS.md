# Repository Guidelines

## Project Structure & Module Organization
`codex-rc` is a single-crate Rust application. `src/main.rs` boots the CLI, starts the Axum server, and launches the Codex app-server client. `src/server.rs` defines the HTTP and WebSocket routes. `src/relay/` contains browser-to-Codex message plumbing, and `src/codex/client.rs` manages the JSON-RPC bridge to `codex app-server`. The mobile/web UI is a single embedded asset at `ui/index.html`. Build outputs land in `target/` and should not be edited manually.

## Build, Test, and Development Commands
Use Cargo for all local workflows:

- `cargo run -- --port 9876` starts the server and serves the embedded UI.
- `cargo run -- --help` shows the CLI surface.
- `cargo build --release` produces an optimized binary.
- `cargo test` runs the current test suite. At the moment, it builds successfully but there are no Rust tests yet.
- `cargo clippy --all-targets --all-features -- -D warnings` is the strict lint check used by this repo.
- `cargo fmt` formats the codebase. Run it before submitting; `cargo fmt --check` currently reports formatting drift in the checked-in source.

## Coding Style & Naming Conventions
Follow standard Rust formatting with 4-space indentation and `rustfmt`. Use `snake_case` for functions and modules, `UpperCamelCase` for types and enums, and keep transport message types in `src/relay/channels.rs`. Prefer `anyhow::Result` for fallible top-level flows and `tracing` for runtime diagnostics. Keep UI changes self-contained in `ui/index.html` unless the project is split into separate assets later.

## Testing Guidelines
There is no formal coverage gate yet. Add unit tests beside the relevant module with `#[cfg(test)]` for parser, relay, or protocol logic, and add integration tests under `tests/` when behavior spans modules. Name tests after observable behavior, for example `start_turn_forwards_prompt_to_codex`. For UI-only changes, include manual verification notes covering connect, send, interrupt, and approval flows.

## Commit & Pull Request Guidelines
This workspace snapshot does not include `.git`, so commit conventions cannot be confirmed from history. Use short, imperative commit subjects such as `Add interrupt relay validation`. Pull requests should summarize behavior changes, list verification commands run, link the related issue when available, and attach screenshots or screen recordings for visible UI changes.

## Security & Configuration Tips
`codex app-server` must be available on `PATH`. The server binds to `0.0.0.0`, so test on trusted networks and avoid embedding secrets or machine-specific paths in prompts, logs, or UI defaults.
