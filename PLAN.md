# CloudLedger Implementation Plan

## Summary

CloudLedger is an Android-first accounting app built with Rust and Tauri v2. The
first product target is personal private ledgers plus small-company public
ledgers with user accounts, organization membership, role-based permissions,
approval states, soft deletion, cloud sync, and audit trails.

The MVP should be a usable APK-oriented application, not a landing page or a
desktop-only prototype. Desktop/Linux support exists mainly to speed up local
development and smoke testing.

## Product Scope

- Support login identities, organizations, memberships, personal ledgers, and
  company public ledgers.
- Keep private ledgers visible only to their owners. Organization admins must
  not automatically see member private-ledger entries.
- Treat company public ledgers as a lightweight quasi-financial system:
  sensitive mutations require audit records, deletion is soft deletion, and
  company entries can enter an approval flow.
- Do not implement full tax reporting, bank auto-sync, OCR, enterprise SSO, or
  full double-entry financial statements in the MVP.
- Preserve an upgrade path toward double-entry accounting through journal entry
  and journal line domain types.

## Architecture

- `crates/cloudledger-core`: pure domain model, money handling, ledger scope,
  membership roles, permissions, journal entries, and approval rules.
- `crates/cloudledger-db`: repository traits plus in-memory and SQLite storage
  drafts.
- `crates/cloudledger-service`: application services for posting entries,
  approval decisions, dashboard DTOs, seeded app state, and Tauri-facing use
  cases.
- `crates/cloudledger-server`: cloud sync and authentication scaffold with
  Argon2 password hashing.
- `src-tauri`: Tauri v2 shell exposing Rust commands to the frontend.
- `frontend`: Android-first Vite/TypeScript UI with ledger switching, quick
  entry, transactions, approvals, and audit views.
- `flake.nix`: pinned NixOS development shell for Rust, Tauri Linux
  dependencies, Android SDK/NDK, JDK, Node, and build helpers.

## Implementation Phases

1. Foundation
   - Keep the Rust workspace, Tauri config, frontend app, server crate, and Nix
     shell building from a clean checkout.
   - Use `path:.` flake references because this environment has a read-only
     placeholder `.git` directory.

2. Accounting Core
   - Harden the money type and currency handling.
   - Expand repository-backed services so the Tauri app no longer depends on
     only seeded in-memory data.
   - Add persisted users, organizations, ledgers, accounts, transactions,
     approval requests, audit logs, and sync events.

3. Mobile App MVP
   - Wire the frontend fully to Tauri commands.
   - Keep the UI optimized for Android: single-column layout, quick entry,
     bottom navigation, touch-friendly controls, and clear offline/error states.
   - Build and verify desktop dev mode first, then Android debug APK.

4. Cloud Sync
   - Turn the server scaffold into real APIs for registration, login, refresh
     token rotation, organization membership, public-ledger sync, and audit-log
     upload.
   - Use cloud authority for public ledgers and local encrypted cache for the
     Android app.
   - Keep private ledger sync user-controlled and isolated from organization
     access.

5. Security And Backup
   - Store mobile tokens and local key material through Android Keystore or a
     Tauri-supported equivalent.
   - Add encrypted backup export/import with schema version, KDF parameters,
     and integrity checks.
   - Ensure attachment access is permission-checked and not based on public
     guessable URLs.

6. APK Delivery
   - Run `just android-init` once Tauri Android project generation is needed.
   - Run `just android-build` to produce the APK/AAB build artifacts.
   - Add signing configuration only after debug APK generation is stable.

## Public Interfaces

- Tauri commands:
  - `health`
  - `get_overview`
  - `create_transaction`
- Server routes currently planned:
  - `GET /health`
  - `GET /sync/ping`
  - future auth and sync endpoints for registration, login, refresh, ledger
    pull/push, and audit-log upload.
- Frontend API boundary:
  - browser/mock mode for fast UI development.
  - Tauri invoke adapter for real app runtime.

## Validation Plan

Run the fast local checks:

```bash
cargo fmt --all -- --check
CARGO_TARGET_DIR=target cargo clippy -p cloudledger-core -p cloudledger-db -p cloudledger-service -p cloudledger-server --all-targets -- -D warnings
CARGO_TARGET_DIR=target cargo test -p cloudledger-core -p cloudledger-db -p cloudledger-service -p cloudledger-server
npm run build
npm audit --audit-level=moderate
npm --prefix frontend audit --audit-level=moderate
cargo metadata --no-deps --format-version 1
nix flake metadata path:.
```

Run the full Nix/Tauri checks once the Android/WebKit closure has finished
downloading:

```bash
just doctor
just check
just tauri-dev
just android-build
```

## Current Status

- Rust core, DB repository draft, service layer, server scaffold, frontend app,
  Tauri shell, Nix shell, and docs have been created.
- Rust non-Tauri crates pass formatting, clippy, and tests.
- Frontend production build passes.
- APK generation has not been completed yet because the first `nix develop`
  run was blocked by slow cache downloads for the Android/WebKit closure.

## Assumptions

- "turi" means Tauri v2.
- MVP chooses cloud sync from the first version.
- Public company ledgers use quasi-financial strictness, not full statutory
  accounting.
- Public ledger data is cloud-authoritative; private ledger data remains
  user-isolated and must not be visible to organization admins by default.
- The first APK can be a debug build; release signing can follow after Android
  build stability is proven.
