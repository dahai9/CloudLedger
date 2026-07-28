# CloudLedger

CloudLedger is an Android-first accounting app built with Rust and Tauri. The
first implementation targets personal private ledgers plus company public
ledgers with cloud sync, role-based authorization, approval states, soft
deletes, and audit trails.

## Development

```bash
nix develop
npm install
cargo test --workspace
npm run build
```

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

Run the development cloud server on the LAN:

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

- `[server]`: mobile API bind address, admin bind address, and data directory.
- `[database]`: PostgreSQL URL, pool size, and connection timeout.
- `[admin]`: randomized admin URL path and platform token.
- `[security.login]`: identifier/IP failure limits, time window, and lockout.
- `[security.turnstile]`: Cloudflare site key, secret key, and verification URL.

PostgreSQL is required before the backend starts. Create the database and a
dedicated login, copy `cloudledger-server.example.toml`, then set
`database.url` to that database. CloudLedger creates and versions tables inside
the existing database; it does not create the database or PostgreSQL login.
The relational model is designed before implementation, then frozen as
reviewable SQL migrations in `crates/cloudledger-server/migrations`. The
backend applies pending migrations at startup through SQLx. See
`docs/backend-data-model.md` for the ownership and migration rules.

PostgreSQL is authoritative for server-shared organizations, ledgers,
transactions, audit logs, users, installations, and sessions. SQLite remains
the client-side local/offline cache boundary in `cloudledger-db`; the backend
does not use SQLite as its primary database.

When PostgreSQL has no CloudLedger application metadata, startup imports
existing `ledger-state.json` and `auth-state.json` files from `server.data_dir`
in one database transaction. The legacy files are read-only migration sources
and are never consulted again after database state exists. Back up both files
and PostgreSQL before an upgrade; do not delete the JSON sources until the
import and a server restart have been verified.

The mobile API defaults to `0.0.0.0:8787` so an Android test phone can reach it
on the LAN. The mobile frontend reads its separate runtime backend URL from
`frontend/public/config.js`. Set `apiBaseUrl` there before an Android build, or
edit `dist/config.js` when deploying the web build. An empty value makes the
web development UI use the current page hostname on port `8787`.
`VITE_CLOUDLEDGER_CLOUD_URL` remains a build-time fallback when the runtime
value is empty.

For example, an Android build that reaches the development machine over LAN can
use:

```js
window.__CLOUDLEDGER_CONFIG__ = {
  apiBaseUrl: "http://10.0.0.42:8787",
};
```

The admin backend is intentionally separated from the mobile API. Its
`server.admin_bind_addr` defaults to `127.0.0.1:8788`. For LAN admin testing,
set it to a specific private address such as `10.0.0.42:8788`; the server
rejects `0.0.0.0` and public IPs for this admin port.

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
own organization-admin login, public ledger, and default company bank account.
Organization admins log in with their own email/phone and password and can
manage employees only inside their organization.

New organization-admin accounts are backend-only identities and do not receive
a personal ledger; `POST /auth/login` rejects them. Employee accounts use the
mobile/Web business frontend, belong to one organization only, and cannot log in
to the organization admin backend. Existing persisted `owner` or `admin`
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

Login brute-force protection is shared by the mobile and admin servers. By
default, five failed attempts for one source IP and login identifier within 15
minutes lock that login for 15 minutes; 20 failed attempts from one IP also lock
that source even when identifiers are rotated. Rate-limited responses use HTTP
`429` with `Retry-After`. New and reset passwords must contain 12–128
characters. Existing password hashes remain valid until the password is reset.
Tune the defaults under `[security.login]` in the backend config.

The limits use the direct TCP peer address. Deployments behind a reverse proxy
must enforce equivalent limits at the proxy because forwarded client IP headers
are intentionally not trusted by the application.

Cloudflare Turnstile protects both organization and platform login forms. Put
the widget credentials configured for the admin hostname in
`security.turnstile.site_key` and `security.turnstile.secret_key`. Keep the
secret key only in this backend config; never put it in frontend `config.js`.

The server refuses a non-loopback admin bind unless both keys are configured.
Turnstile may be omitted only for loopback-only local development. When a
reverse proxy exposes a loopback-bound admin server, the application cannot
detect that public exposure, so the keys and proxy-level request limits are
still required for a secure deployment. Turnstile responses must carry the
`admin-login` action and are verified server-side with the direct peer IP.

Business access tokens expire after 15 minutes and use the existing rotating
refresh flow. Refresh tokens expire after 30 days. Organization-admin sessions
expire after 8 hours and require a new login; changing an account password or
account type continues to revoke all of that user's sessions immediately.

The mobile API owns app login and ledger operations:

- `POST /auth/login`
- `POST /auth/refresh`
- `GET /auth/me`
- `POST /auth/logout`
- `GET /app/overview`
- `POST /app/transactions`
- `POST /app/approvals/decide`

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
