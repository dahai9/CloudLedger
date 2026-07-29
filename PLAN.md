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
- Target small organizations with one or two business owners and roughly two to
  five employees. Every business account can record transactions.
- Keep private ledgers visible only to their owners. Organization admins must
  not automatically see member private-ledger entries.
- Keep organization administration separate from business work: backend-only
  administrators manage accounts, while business owners approve and pay public
  expense applications and employees confirm receipt.
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
  for the client-side local/offline cache boundary.
- `crates/cloudledger-service`: application services for posting entries,
  approval decisions, dashboard DTOs, multi-organization app state, server-side
  organization membership management, and Tauri-facing use cases.
- `crates/cloudledger-server`: cloud sync and authentication scaffold with
  Argon2 password hashing, PostgreSQL-authoritative shared storage, model-driven
  SQLx file migrations, plus a separate admin backend for organization and
  account relationship management.
- `src-tauri`: Tauri v2 shell exposing Rust commands to the frontend.
- `frontend`: Android-first Vite/TypeScript UI with server-backed login, a
  single logged-in account, ledger switching, quick entry, transactions,
  approvals, and audit views.
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
   - Wire the frontend to the mobile server API for login, overview, quick
     entry, approval decisions, owner-only financial analysis, and audit views.
   - Keep the UI optimized for Android: single-column layout, quick entry,
     bottom navigation, touch-friendly controls, and clear offline/error states.
   - Build and verify desktop dev mode first, then Android debug APK.

4. Cloud Sync
   - Turn the server scaffold into real APIs for admin-created accounts, login,
    refresh token rotation, public-ledger sync, and audit-log upload.
   - Keep organization membership and account relationship management on the
     server-side admin backend, never in the Android app.
   - Use cloud authority for public ledgers and local encrypted cache for the
     Android app.
   - Keep private ledger sync user-controlled and isolated from organization
     access.

5. Security And Backup
   - Rate-limit failed business, organization-admin, and platform-token
     authentication by direct peer IP and normalized login identifier.
   - Require 12–128 character passwords for new accounts and password resets,
     while keeping existing hashes login-compatible.
   - Return `429` plus `Retry-After` during login lockout and apply no-store,
     anti-framing, MIME-sniffing, referrer, permissions, and CSP headers to the
     admin backend.
   - Expire business access tokens after 15 minutes, rotating refresh tokens
     after 30 days, and organization-admin sessions after 8 hours.
   - Generate and persist a high-entropy admin path during initialization;
     never expose the management UI or APIs at the fixed `/admin` route.
   - Require Cloudflare Turnstile for organization and platform login whenever
     the admin listener is not loopback-only. Exchange the raw platform token
     for a revocable eight-hour session instead of accepting it on every API
     request.
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
  - `decide_approval`
- Server routes currently planned:
  - `GET /health`
  - `GET /ready`
  - `GET /sync/ping`
  - `POST /auth/login`
  - `POST /auth/refresh`
  - `GET /auth/me`
  - `POST /auth/logout`
  - `GET /app/overview`
  - `GET /app/analytics?ledgerId=<uuid>&months=6`
  - `POST /app/transactions`
  - `POST /app/approvals/decide`
  - `POST /app/payments/mark-paid`
  - `POST /app/payments/confirm-receipt`
  - `GET /{admin_path}`
  - `GET /{admin_path}/api/security`
  - `POST /{admin_path}/api/login`
  - `POST /{admin_path}/api/platform-login`
  - `POST /{admin_path}/api/logout`
  - `GET /{admin_path}/api/me`
  - `GET /{admin_path}/api/organizations`
  - `POST /{admin_path}/api/organizations`
  - `GET /{admin_path}/api/organizations/:organization_id/members`
  - `POST /{admin_path}/api/organizations/:organization_id/members`
  - `PATCH /{admin_path}/api/organizations/:organization_id/members/:membership_id`
  - `PATCH /{admin_path}/api/organizations/:organization_id/members/:membership_id/password`
  - `DELETE /{admin_path}/api/organizations/:organization_id/members/:membership_id`
  - future sync endpoints for ledger pull/push and audit-log upload.
- Server ports:
  - Backend deployment settings are consolidated in
    `.cloudledger-server/config.toml`, with `--config <path>` available for an
    explicit alternate file.
  - Mobile API uses `server.api_bind_addr`, defaulting to `0.0.0.0:8787` for
    Android LAN testing.
  - Admin backend uses `server.admin_bind_addr`, defaulting to `127.0.0.1:8788`;
    LAN admin testing must bind a specific private IP such as `10.0.0.42:8788`
    and configure Cloudflare Turnstile. Binding admin to `0.0.0.0` or public IPs
    is rejected.
- Frontend API boundary:
  - browser/mock mode for fast UI development.
  - HTTP adapter for the real app runtime, pointed at the runtime
    `frontend/public/config.js` `apiBaseUrl`. The build-time
    `VITE_CLOUDLEDGER_CLOUD_URL` value remains a fallback.

## Validation Plan

Run the fast local checks:

```bash
cargo fmt --all -- --check
CARGO_TARGET_DIR=target cargo clippy -p cloudledger-core -p cloudledger-db -p cloudledger-service -p cloudledger-server --all-targets -- -D warnings
CARGO_TARGET_DIR=target cargo test -p cloudledger-core -p cloudledger-db -p cloudledger-service -p cloudledger-server
CLOUDLEDGER_TEST_DATABASE_URL=postgres://cloudledger:password@127.0.0.1:15433/cloudledger_test CARGO_TARGET_DIR=target cargo test -p cloudledger-server storage::postgres::tests::postgres_storage_end_to_end -- --ignored --test-threads=1
npm run build
npm audit --audit-level=moderate
npm --prefix frontend audit --audit-level=moderate
cargo metadata --no-deps --format-version 1
nix flake metadata path:.
```

Run the full Nix/Tauri checks:

```bash
just doctor
just check
just tauri-dev
nix develop path:. -c npm run tauri:android:build -- --debug --target aarch64
```

## Current Status

- Rust core, DB repository draft, service layer, server scaffold, frontend app,
  Tauri shell, Nix shell, and docs have been created.
- The Android app uses one current account per install/session and no longer
  exposes account switching or organization membership management on the phone.
- The mobile API now provides login, refresh, `me`, logout, server overview,
  transaction creation, and approval decisions. Registration is not exposed on
  the mobile/user frontend; admins create accounts and initial credentials from
  the separate admin backend. Each login is bound to the app installation id, so
  one app install cannot switch to another account through the mobile UI.
- Account and organization relationships are managed by the server-side admin
  backend on the separate admin port. The admin port defaults to localhost and
  can only be moved to loopback, link-local, or private LAN addresses.
- The platform token can create and list multiple organizations. Each new
  organization receives an independent backend-only administrator account, an
  organization public ledger, and a default company bank account.
- Organization administrators authenticate with admin-only sessions and can
  manage employees only inside their assigned organization. They cannot log in
  to the business frontend, and employee accounts cannot log in to the admin
  backend or be shared across organizations. Existing persisted `owner/admin`
  accounts migrate to this split at server startup.
- Each user sees their own private ledger first, while permitted organization
  members can see the configured organization's public ledger through
  membership.
- Server-side transaction and approval routes no longer trust a renderer-
  provided actor user ID; they inject the authenticated access-token user before
  authorization.
- Business membership now has two active roles: `business_owner` (老板) and
  `employee` (员工). Legacy approver roles migrate to business owners; legacy
  accountant/member/viewer roles migrate to employees. Technical `owner/admin`
  memberships remain backend-only and have no business-ledger permissions.
- Employee public expenses follow a reimbursement workflow: submitted expenses
  await a business owner, approval changes them to approved/pending-payment,
  marking payment posts the expense to the public account, and the original
  applicant closes the workflow by confirming receipt. Approval alone does not
  reduce the public-account balance.
- In a one-owner organization, that owner's public entries auto-approve because
  no independent business approver exists. With two owners, an owner's entry
  requires the other owner; self-approval remains forbidden. Rejection requires
  a reason, and submission, approval, payment, and receipt all write audit
  events.
- Business owners have an organization-public-ledger analysis view for 3, 6,
  or 12 months. The backend returns account balances, actual cash flow, prior
  period comparison, monthly trend, workflow exposure, member spending, and
  largest expenses. Employees and backend admins are denied by service-layer
  authorization. Paid time is the expense cash-flow date; approval alone never
  counts as money spent.
- Public-ledger approval validation uses admin-created members in real server
  flows; seeded Acme/Alice/Bob data remains only as an explicit service-layer
  test fixture.
- Audit logs are returned only when the actor has `ViewAuditLog` permission for
  that ledger. Business owners and employees can inspect their shared public
  ledger's audit trail; backend-only organization administrators cannot access
  the business ledger.
- MVP transaction creation rejects unsupported transfer entries and currency
  mismatches between the transaction and selected account, keeping posted
  balances deterministic.
- PostgreSQL is authoritative for backend domain and authentication data. Its
  typed relational schema is designed before implementation and versioned in
  immutable SQLx migration files; organization plus authentication changes
  commit in one transaction.
- Existing `ledger-state.json` and `auth-state.json` files are read-only import
  sources when PostgreSQL is empty. After import, database state wins and the
  JSON files are not read again. SQLite remains reserved for the client-side
  local/offline cache.
- The frontend shows dev-cloud status from runtime `config.js`, then falls back
  to `VITE_CLOUDLEDGER_CLOUD_URL`. With neither configured, a web build uses the
  current page hostname on port `8787`. The app checks `/ready` and displays a
  short server ID so the active development cloud can be identified.
- The development server persists its cloud identity under
  `.cloudledger-server/server-id` by default, or under the backend config's
  `server.data_dir` when changed.
- Debug APK generation and installation have been proven on a connected Android
  phone. The validated debug artifact path is
  `src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk`.
- Phone-side validation should use the developer machine's current private LAN
  address and the phone WLAN address reported by `adb shell ip -4 addr`.

## Assumptions

- "turi" means Tauri v2.
- MVP chooses cloud sync from the first version.
- Public company ledgers use quasi-financial strictness, not full statutory
  accounting.
- Public ledger data is cloud-authoritative; private ledger data remains
  user-isolated and must not be visible to organization admins by default.
- The first APK can be a debug build; release signing can follow after Android
  build stability is proven.
