# CloudLedger

CloudLedger is an Android-first accounting app built with Rust and Tauri. The
first implementation targets personal private ledgers plus company public
ledgers with cloud sync, role-based authorization, approval states, soft
deletes, and audit trails.

## Development

```bash
nix develop path:. -c npm install
nix develop path:. -c cargo test --workspace --locked
nix develop path:. -c npm run build
```

The repository uses the pinned Rust toolchain from `flake.nix`; run Rust and
Tauri commands through `nix develop` (or the `just` wrappers) so a stale host
`rustup` installation cannot be selected accidentally.

Useful commands are wrapped in `justfile`:

```bash
just check
just tauri-dev
just android-build
```

The Android target is built through Tauri v2:

```bash
nix develop path:. -c npm run tauri:android:build -- --debug --target aarch64
```

Use debug builds during development. Do not run a release Android build unless
release signing or publication is the explicit target.

Run the development cloud server on loopback:

```bash
nix develop path:. -c cargo run -p cloudledger-server
```

All backend deployment settings live in one TOML file. The default path is
`.cloudledger-server/config.toml`; the server creates and validates it on first
startup and restricts it to mode `0600` on Unix because it contains secrets.
Use `cloudledger-server.example.toml` as the complete schema reference. A
different file can be selected explicitly:

```bash
nix develop path:. -c cargo run -p cloudledger-server -- --config /etc/cloudledger/server.toml
```

The generated file contains these sections:

- `[server]`: run mode, loopback listeners, public HTTPS URLs, and data directory.
- `[database]`: runtime PostgreSQL URL, migration policy, pool size, and timeout.
- `[admin]`: randomized admin URL path and platform token.
- `[security.login]`: identifier/IP failure limits, time window, and lockout.
- `[security.turnstile]`: Cloudflare site key, secret key, and verification URL.
- `[security.network]`: trusted proxy CIDRs and exact CORS origins.
- `[security.audit]`: audit HMAC key ID and 32-byte signing/identifier keys.

PostgreSQL is required before the backend starts. Production uses a dedicated
`cloudledger_bootstrap` operator plus separate `cloudledger_migration` and
`cloudledger_runtime` logins. Bootstrap them with `deploy/postgres_roles.sql`,
put only the runtime URL in `database.url`, then run
the one-time migration with the migration URL in
`CLOUDLEDGER_MIGRATION_DATABASE_URL`:

```bash
CLOUDLEDGER_MIGRATION_DATABASE_URL='postgres://cloudledger_migration:...@127.0.0.1/cloudledger' \
  cloudledger-server migrate --config /etc/cloudledger/server.toml
```

Production (`server.mode = "reverse_proxy"`) requires
`database.auto_migrate = false`; the long-running service never receives the
migration credential. Development may enable automatic migrations. The
relational model is frozen as reviewable SQL migrations in
`crates/cloudledger-server/migrations`. See `docs/backend-data-model.md` and
`docs/security-hardening.md` for the data and deployment rules.

PostgreSQL is authoritative for server-shared organizations, ledgers,
transactions, audit logs, users, installations, and sessions. SQLite remains
the client-side local/offline cache boundary in `cloudledger-db`; the backend
does not use SQLite as its primary database.

## Production Operations

CloudLedger v0.1.5 has one production-administrator entry point:

```bash
sudo ./deploy/cloudledger-ops.sh
```

Normal administration is performed only through its numeric, multi-level
menus; `0` always returns to the previous menu or exits. The script prompts for
and hides secrets, shows the impact of destructive actions, and requires an
additional backup-number confirmation before a restore. Public subcommands are
not supported. The hidden `--internal` tasks are implementation details used
only by systemd for backup, health checks, restore drills, and Cloudflare
firewall refreshes.

The complete-install wizard stages versioned deployment assets under
`/opt/cloudledger` and stores private operations settings in
`/etc/cloudledger/ops.env`, backend settings in
`/etc/cloudledger/server.toml`, and rclone settings in
`/etc/cloudledger/rclone.conf`. It deploys four matching, explicit GHCR tags
for the server, PostgreSQL, Caddy, and `network-anchor`, never `latest`.
The shared `network-anchor` publishes HTTP only on
`127.0.0.1:18080`, leaving any existing public port-80 service untouched; HTTPS
uses host port `443` and is restricted by nftables to Cloudflare's published IP
ranges. The firewall unit does not require or control `docker.service`, so its
failure cannot stop unrelated Docker workloads. The admin host mapping is
`127.0.0.1:8788:18788`; `network-anchor` relays container port `18788` to the
backend's namespace-loopback `127.0.0.1:8788`, and administrators connect only
through an SSH tunnel.

Database migration runs through the dedicated Compose `migration` profile
after PostgreSQL is healthy and before the long-running backend or Caddy starts.
Backups use a real non-empty `pg_dump -Fc`. Both local and rclone objects remain
hidden `.new` candidates until local validation and remote download comparison
succeed, then become visible through an atomic rename. Restore accepts only a
canonical regular file in the protected backup directory, binds the manifest ID
and UTC creation time to that filename, and validates the
exact nine-member archive, size limits, checksums, normalized `ops.env`, four
matching GHCR tags from the currently trusted owner, a canonical `server.toml`,
a matching Origin CA certificate/private-key pair with the required SAN, and the
current version's trusted Compose and Caddy templates. Candidate Compose parsing
removes all inherited `CLOUDLEDGER_*` variables. A weekly drill restores the
latest verified backup into a temporary database and validates the target
image's migration level, core tables, the audit chain, and successful temporary
database cleanup. Deployment also probes the configured Turnstile secret
directly against Cloudflare's `siteverify` endpoint before reporting success.

完整的中文、菜单驱动生产手册（主机要求、部署顺序、定时任务、备份内容、恢复边界和
验收门槛）见 [`docs/backend-deployment.md`](docs/backend-deployment.md)。

When PostgreSQL has no CloudLedger application metadata, startup imports
existing `ledger-state.json` and `auth-state.json` files from `server.data_dir`
in one database transaction. The legacy files are read-only migration sources
and are never consulted again after database state exists. Back up both files
and PostgreSQL before an upgrade; do not delete the JSON sources until the
import and a server restart have been verified.

Both API and admin listeners default to loopback. Production requires loopback
listeners and HTTPS public URLs behind `deploy/Caddyfile`. The mobile frontend
reads its separate runtime backend URL from `frontend/public/config.js`. Set
`apiBaseUrl` there before an Android build, or
edit `dist/config.js` when deploying the web build. An empty value makes the
web development UI use the current page hostname on port `8787`.
`VITE_CLOUDLEDGER_CLOUD_URL` remains a build-time fallback when the runtime
value is empty.

In development only, LAN HTTP requires both a specific LAN bind address and
`server.allow_insecure_lan = true`; startup prints a high-visibility warning.
For example, an Android debug build can then use:

```js
window.__CLOUDLEDGER_CONFIG__ = {
  apiBaseUrl: "http://10.0.0.42:8787",
};
```

The admin backend is intentionally separated from the mobile API and defaults
to `127.0.0.1:8788`. Development LAN binding follows the same explicit
`allow_insecure_lan` opt-in. Production never binds either service to a LAN or
public address.

On first initialization the server generates a high-entropy path such as
`manage-0123456789abcdef0123456789abcdef` plus a platform token and writes both
to the `[admin]` section of the config file. The fixed `/admin` route
intentionally returns `404`. A configured `admin.path` must be one 16-128
character path segment; deployments should keep it unguessable. Existing
`admin-path` and `admin-token` files are imported once when upgrading to the
unified config, so established credentials remain valid.

The randomized admin page has separate platform and organization entry points.
The raw platform token must first be exchanged for a revocable eight-hour
platform session; it is not accepted as an API bearer token. The platform
session creates and lists organizations. Every organization is created with its
own organization-admin login, public ledger, and standard 微信、支付宝、银行账户、
现金 accounts.
Organization admins log in with their own email/phone and password and can
manage employees only inside their organization.

New organization-admin accounts are backend-only identities and do not receive
a personal ledger; `/auth/tauri/login` and `/auth/web/login` reject them.
Employee accounts use the mobile/Web business frontend, belong to one
organization only, and cannot log in to the organization admin backend.
Existing persisted `owner` or `admin`
membership accounts are migrated to backend-only organization admins when the
server starts.

Business accounts use only two roles: `business_owner` (老板) and `employee`
(员工). The product is optimized for a small team with one or two owners and a
few employees; these sizes are operating targets rather than database hard
caps. Every business account can record personal and permitted public-ledger
transactions, while only a business owner can approve public applications or
mark an approved expense as paid.

Public expense reimbursement has a separate approval state and payment state:

1. An employee submits a public expense: `submitted`.
2. A different business owner approves it: `approved` + `pending_payment`.
3. A business owner sends the money and marks it paid:
   `paid_pending_receipt`. The public-account balance changes at this step.
4. The original applicant confirms receipt: `received`.

Approval does not reduce the public-account balance, and receipt confirmation
does not post the expense a second time. A sole owner's public entry is
auto-approved because no independent business approver exists. When two owners
exist, one owner's entry must be approved by the other owner. Every transition
is included in the shared public-ledger audit trail.

Business owners also have a public-ledger financial analysis view with 3, 6,
and 12 month ranges. It reports current account balances, actual income and
paid expenses, net cash flow, monthly trends, open approval/payment exposure,
member spending, and the largest paid expenses. Employees cannot access this
view or its API. Expense cash flow uses `paid_at`; approval alone is shown as a
future payment commitment and never counted as money already spent.

The transaction view loads one month at a time instead of growing into an
unbounded list. Users can switch among months that contain ledger activity.
Quick entry uses the four standard account choices (微信、支付宝、银行账户、现金),
and any authorized ledger member can add reusable income or expense categories
for that ledger.

Login brute-force protection is shared by the mobile and admin servers. By
default, five failed attempts for one source IP and login identifier within 15
minutes lock that login for 15 minutes; 20 failed attempts from one IP also lock
that source even when identifiers are rotated. Rate-limited responses use HTTP
`429` with `Retry-After`. New and reset passwords must contain 12–128
characters. Existing password hashes remain valid until the password is reset.
Tune the defaults under `[security.login]` in the backend config.

Login failures and security request buckets are stored in PostgreSQL so all
server instances share them. Identifiers are stored only as keyed HMAC values.
The application accepts `X-Forwarded-For` and `X-Forwarded-Proto` only from
`security.network.trusted_proxy_cidrs`; untrusted peers cannot spoof their
source. Caddy overwrites forwarding headers with the direct client address.

Cloudflare Turnstile always protects organization and platform login forms and
is required for production startup. Business login does not send a challenge
until the third failed attempt returns `428 turnstile_required`; the client then
loads the widget and submits its one-time token. Put the widget credentials in
`security.turnstile.site_key` and `security.turnstile.secret_key`. Keep the
secret key only in this backend config; never put it in frontend `config.js`.

Turnstile may be omitted only for loopback-only local development. Business
tokens must carry the `business-login` action; admin tokens use `admin-login`.
Both are verified server-side with the trusted client IP.

Business access tokens expire after 15 minutes and refresh tokens after 30
days. Refresh tokens are single-use; replay revokes the entire session family.
PostgreSQL stores only SHA-256 token digests. Tauri keeps access tokens only in
memory. Android encrypts the refresh token with a non-exportable Keystore
AES-GCM key in the no-backup directory. Desktop uses the OS credential store
when available and otherwise falls back to a memory-only session. Development
Web login uses a Secure, HttpOnly, SameSite=Strict refresh cookie and a memory
access token; production disables Web login. No session is stored in browser
`localStorage`.

Organization-admin and platform sessions expire after eight hours and require
a new login; changing an account password or account type revokes all of that
user's sessions immediately.

The mobile API owns app login and ledger operations:

- `POST /auth/tauri/login`
- `POST /auth/tauri/refresh`
- `GET /auth/tauri/me`
- `POST /auth/tauri/logout`
- `GET /app/overview`
- `GET /app/analytics?ledgerId=<uuid>&months=6`
- `GET /app/transactions?ledgerId=<uuid>&month=YYYY-MM`
- `POST /app/categories`
- `POST /app/transactions`
- `POST /app/approvals/decide`
- `POST /app/payments/mark-paid`
- `POST /app/payments/confirm-receipt`
- `POST /app/transactions/void` (business owner only; approved organization-public transactions)

Development Web auth uses `/auth/web/login|refresh|logout`. The legacy
`/auth/login|refresh|logout` endpoints return `426 client_upgrade_required` for
one compatibility release.

Login binds the server session to the app installation id. The Android UI does
not expose registration, account switching, or organization membership
management; account creation and organization membership are managed only
through the admin backend.

The server persists its development identity in `.cloudledger-server/server-id`
by default. Change `server.data_dir` in the backend config to move that state.
Business and authentication records are persisted in the configured PostgreSQL
database, not in this directory after migration.

## Android Smoke Test

With a phone connected through ADB:

```bash
adb devices
adb shell curl -sS --connect-timeout 5 http://10.0.0.42:8787/ready
adb shell pm clear com.cloudledger.app
adb install -r src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk
adb shell am start -n com.cloudledger.app/.MainActivity
adb logcat -d | grep CloudLedger
```

Verify that the phone app shows the login screen when no session exists, binds
the installation after login, shows one current account only, displays
that account's private ledger plus permitted public ledgers, and does not expose
organization membership management on Android.

Use the server-side admin backend to manage organization/account relationships:

```bash
nix develop path:. -c cargo run -p cloudledger-server
```

Read `admin.path` from `.cloudledger-server/config.toml`, then open
`http://127.0.0.1:8788/<admin.path>`. Use the platform-token tab with
`admin.token` from the same file to create organizations. Afterwards, each
organization administrator uses the organization-account tab to create and
manage that organization's employee accounts.

For the public ledger approval smoke tests:

1. Submit a public expense as an employee and confirm it remains pending without
   changing the public-account balance.
2. Log in as a business owner on another installation, approve it, and confirm
   the state is approved/pending-payment while the balance remains unchanged.
3. Mark it paid as a business owner and confirm the public-account balance is
   reduced exactly once.
4. Return to the applicant account, confirm receipt, and verify the final state
   and submission/approval/payment/receipt audit actors.
