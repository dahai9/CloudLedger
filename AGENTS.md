# Repository Guidelines

## Project Structure & Module Organization

CloudLedger is a Rust workspace with a Tauri v2 client. Shared domain types live in `crates/cloudledger-core`; database access and migrations are in `crates/cloudledger-db` and `crates/cloudledger-server/migrations`; HTTP services and the server binary are in `crates/cloudledger-service` and `crates/cloudledger-server`. The web client is under `frontend/`, while Tauri configuration, native bindings, Android generation, and icons are under `src-tauri/`. Deployment assets, the menu-driven operations toolbox, Docker files, systemd units, and shell tests are under `deploy/`. Design, security, and deployment notes belong in `docs/`.

## Build, Test, and Development Commands

Use the pinned environment before development: `nix develop`.

- `just check` runs Rust formatting, Clippy, workspace tests, dependency installation, and the frontend production build.
- `just test` runs `cargo test --workspace`; use `nix develop path:. -c cargo test --workspace --locked` to match CI exactly.
- `just tauri-dev` starts the desktop development client; `just client-web` starts the frontend dev server.
- `just android-build` builds the Tauri Android app. Use debug builds during development; release signing is CI/release work.
- `bash deploy/tests/cloudledger-ops-test.sh` exercises the production operations toolbox; `bash deploy/tests/release-workflow-test.sh` checks release-source rules.

## Coding Style & Naming Conventions

Run `cargo fmt --all` and keep Clippy warning-free with `-D warnings`. Follow idiomatic Rust naming: `snake_case` for functions/modules, `UpperCamelCase` for types, and `SCREAMING_SNAKE_CASE` for constants. Shell scripts use Bash strict mode, quoted variables, explicit temporary paths, and numeric menu-only public entry points. Keep secrets out of source, fixtures, logs, and commit messages.

## Testing Guidelines

Add Rust tests beside the module or in the crate's existing test modules; run the narrow test first, then the workspace suite. Deployment changes require the shell suite and release-policy test. Changes involving PostgreSQL, migrations, backups, or restore drills must validate the real boundary where practical, not only command mocks.

## Commit & Pull Request Guidelines

Use concise Conventional Commit-style subjects such as `feat:`, `fix:`, or `ci:`. Keep commits focused and run `git diff --check` before committing. PRs should explain behavior and risk, list verification commands, and include screenshots for UI changes. Never rewrite an existing release tag. Commits on `main` publish numbered Alpha prereleases such as `v0.1.8.alpha.1`; formal `vX.Y.Z` releases must come from the matching `release/vX.Y.Z` branch and must publish explicit, matching GHCR image tags.
