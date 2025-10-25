# Repository Guidelines

## Project Structure & Module Organization
- Core service lives in `src/` with domain modules: `database/`, `logic/`, `graphql/`, `grpc/`, `scheduler/`, `middleware/`.
- Shared config is defined in `src/config.rs` with defaults in `config.toml` and logging in `log4rs.yaml`.
- Database migrations live in `migrations/`, proto contracts in `proto/`, integration suites mirror domains under `tests/`, and helper scripts sit in `examples/` plus `test_jwt.sh`.

## Build, Test, and Development Commands
- `cargo check` or `cargo build --release` compile the Actix + gRPC binary; `build.rs` regenerates gRPC code automatically.
- `cargo run -- --config ./config.toml` starts REST, GraphQL, and gRPC endpoints with local settings.
- `cargo test`, `cargo fmt`, `cargo clippy --all-targets --all-features -D warnings`, and `./test_jwt.sh` guard regressions and style.

## Coding Style & Naming Conventions
- Use `rustfmt` defaults (4 spaces, trailing commas, module reordering); never hand format Rust files.
- Modules and files stay snake_case, types/enums/traits stay PascalCase, and async functions read as verbs.
- GraphQL fields align with database column snake_case; gRPC RPC names stay CamelCase and proto edits happen only in `proto/*.proto`.

## Testing Guidelines
- Keep fast unit tests near code in `mod tests`; broader flows live under `tests/` per domain.
- Name integration files `<domain>_tests.rs` or `<feature>_test.rs` and use `tests/database` fixtures for Postgres state.
- Run `cargo test -- --ignored` before merging whenever `subscription_event.rs` or other long-running cases change.

## Commit & Pull Request Guidelines
- Follow Conventional Commit prefixes like `feat:`, `refactor:`, `fix:` as seen in recent history (`refactor: Migrate evaluation subscriptions...`).
- Reference issues or tickets in the body and flag migrations/config updates so reviewers can plan rollouts.
- Pull requests need a summary, testing notes, screenshots or GraphQL snippets when behavior shifts, and tagged owners for affected areas.

## Security & Configuration Tips
- Secrets live outside the repo; `Config::load()` reads environment overrides so never hard-code credentials.
- Rotate JWT secrets through the provided logic and rerun `./test_jwt.sh` after touching auth flows.
- Prefer per-developer Postgres schemas; adjust `config.toml` or env vars rather than editing SQL directly.
