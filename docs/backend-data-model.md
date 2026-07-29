# Backend Data Model

The PostgreSQL schema is designed from the domain and persistence model first,
then recorded as immutable SQL files in
`crates/cloudledger-server/migrations`. The backend applies pending migrations
through SQLx when it starts. Application types describe the desired model;
migration files are the authoritative, reviewable history for deployed
databases.

The PostgreSQL database and login must already exist. CloudLedger owns the
tables listed below inside that database, creates them on first startup, and
records applied versions and checksums in SQLx's `_sqlx_migrations` table.

## Migration Rules

- Design domain and persistence changes before writing database DDL.
- Generate and review a new numbered SQL file for every schema transition.
- Never edit a migration after it has been deployed; add the next migration.
- Put data backfills, renames, and phased constraint changes in explicit
  migrations instead of inferring them from the current Rust types.
- Keep `build.rs` watching the migration directory so newly added files are
  embedded in release binaries on stable Rust.
- Validate both empty-database creation and supported upgrade paths before
  release.

## Ownership

- PostgreSQL is authoritative for shared server data.
- SQLite remains a client-side cache and offline repository boundary.
- The legacy `ledger-state.json` and `auth-state.json` files are import sources
  only. They are loaded without modification, and a database with existing
  application metadata is never overwritten from JSON.

## Domain Model

- `domain_users`: ledger-visible user profiles.
- `organizations`: organization identity and creator.
- `organization_memberships`: one role per user and organization. `owner` and
  `admin` are backend-only administration markers; `business_owner` and
  `employee` are business roles.
- `ledgers`: personal or organization-public ledger ownership.
- `financial_accounts`: account and opening balance within a ledger.
- `transactions`: accounting event, approval state, independent payment state,
  and approval/payment/receipt actors and timestamps.
- `audit_logs`: actor, action, resource, and immutable event time.

## Authentication Model

- `auth_users`: password hash, account kind, and organization-admin scope.
- `auth_installations`: one app installation bound to one business identity.
- `auth_sessions`: rotating business sessions and organization-admin sessions.

Platform sessions and brute-force counters remain intentionally ephemeral. They
contain no business records and are reset when the process restarts.

## Write Semantics

The current service layer mutates in-memory domain objects and persists owned
snapshots as typed relational rows. A server-wide asynchronous write gate keeps
snapshot commits in mutation order. Organization and authentication snapshots
use one PostgreSQL transaction, and deferred organization-admin foreign keys
allow existing organization rows to be rebuilt without a temporary integrity
violation.

This is the compatibility adapter for the current service architecture. Future
high-volume work can replace full snapshot materialization with entity-level
repositories without changing the relational model or moving authority back to
SQLite.

## Financial Analysis Model

Financial analysis is derived from authoritative ledger rows, so it does not
add aggregate tables or a schema migration. The service currently materializes
the small-team snapshot and computes owner-only public-ledger summaries for 3,
6, or 12 calendar months.

- Approved income is recognized at `approved_at`, falling back to
  `occurred_at` for legacy records.
- Public expense cash flow is recognized at `paid_at`. Legacy settled records
  fall back to `received_at` and then `occurred_at`.
- Submitted expenses and approved-but-unpaid expenses are current workflow
  exposure, not historical cash outflow.
- `paid_pending_receipt` expenses have already reduced the account balance and
  are included in cash outflow while remaining visible as unsettled receipts.
- Current account balances include opening balances and every transaction that
  affects the ledger balance; the selected range does not limit this snapshot.
- Only a `business_owner` on the organization public ledger has
  `ViewFinancialAnalytics`; employee and backend-admin requests are rejected by
  the service authorization layer.

This in-memory aggregation is appropriate for the target organization size.
If transaction volume grows materially, the same response model can be backed
by indexed PostgreSQL aggregate queries without changing its accounting
semantics.

## Invariants

- Organization membership is unique by organization and user.
- Personal ledgers have an owner and no organization.
- Organization ledgers have an organization and no personal owner.
- Email, phone, access token, and non-empty refresh token values are unique.
- Money uses signed 64-bit minor units plus a three-letter currency code.
- Organization administration changes domain and authentication rows in one
  PostgreSQL transaction.
- Backend-only `owner/admin` memberships have no business-ledger permissions.
- Employee public expenses move through `submitted`, then
  `approved/pending_payment`, `paid_pending_receipt`, and `received`.
- Approval does not post a reimbursable expense. Marking it paid posts it once;
  receipt confirmation records settlement without changing the balance again.
- Only a business owner can approve or mark payment, self-approval is forbidden,
  and only the original applicant can confirm receipt.
- Payment-state actor/timestamp combinations are constrained in PostgreSQL and
  indexed by ledger plus payment state.

## Legacy Import

On an empty CloudLedger database, startup checks `server.data_dir` for
`ledger-state.json` and `auth-state.json`. Either file may be present; missing
state uses an empty service. The combined snapshot is committed atomically and
subsequent starts load PostgreSQL only.

Before migration, back up the JSON files and the target database. After the
first successful start, restart the backend and verify `/ready`, login, and the
organization list before retiring the JSON copies.

## Integration Test

Point the ignored integration test only at a disposable database because it
drops and recreates the `public` schema:

```bash
CLOUDLEDGER_TEST_DATABASE_URL=postgres://cloudledger:password@127.0.0.1:15433/cloudledger_test \
  CARGO_TARGET_DIR=target cargo test -p cloudledger-server \
  storage::postgres::tests::postgres_storage_end_to_end -- \
  --ignored --test-threads=1
```

The test covers SQL migrations, empty initialization, read-only JSON import,
rollback after a deferred foreign-key failure, atomic organization/auth writes,
and reload from PostgreSQL.
