# CloudLedger Architecture

CloudLedger uses a Rust domain core shared by the Android Tauri app and the
cloud sync service.

## Boundaries

- `cloudledger-core`: pure domain types and permission rules.
- `cloudledger-db`: SQLite schema and local persistence primitives.
- `cloudledger-service`: accounting use cases, approval transitions, audit
  event creation, and read models.
- `cloudledger-server`: cloud-sync/authentication scaffold.
- `src-tauri`: mobile/desktop shell exposing Rust commands to the frontend.
- `frontend`: Android-first UI built with Vite and TypeScript.

The frontend never performs ledger authorization by itself. UI state is only a
presentation layer over Rust commands or cloud APIs.

## Ledger Model

Private ledgers belong to a single user. Organization public ledgers belong to
an organization and are guarded by membership roles.

Public ledger mutations produce audit events and use soft deletion. The MVP
uses approval states without implementing full double-entry accounting.
