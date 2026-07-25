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

The mobile API listens on `CLOUDLEDGER_BIND_ADDR`, defaulting to
`0.0.0.0:8787` so the Android test phone can reach it on the LAN. The mobile
frontend reads its runtime backend from `frontend/public/config.js`. Set
`apiBaseUrl` there before an Android build, or edit `dist/config.js` when
deploying the web build. An empty value makes the web development UI use the
current page hostname on port `8787`. `VITE_CLOUDLEDGER_CLOUD_URL` remains a
build-time fallback when the runtime value is empty.

For example, an Android build that reaches the development machine over LAN can
use:

```js
window.__CLOUDLEDGER_CONFIG__ = {
  apiBaseUrl: "http://10.0.0.42:8787",
};
```

The admin backend is intentionally separated from the mobile API. It listens on
`CLOUDLEDGER_ADMIN_BIND_ADDR`, defaulting to `127.0.0.1:8788`. For LAN admin
testing bind it to a specific private address, for example `10.0.0.42:8788`;
the server rejects `0.0.0.0` and public IPs for this admin port.

On first initialization the server generates a high-entropy path such as
`manage-0123456789abcdef0123456789abcdef` and stores it in the server data
directory's `admin-path` file with mode `0600` on Unix. The fixed `/admin`
route intentionally returns `404`. `CLOUDLEDGER_ADMIN_PATH` can override the
generated value with one 16-128 character path segment, but deployments should
keep it unguessable. The platform token comes from `CLOUDLEDGER_ADMIN_TOKEN` or
the data directory's `admin-token` file, which is also restricted to `0600`.

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

Login brute-force protection is shared by the mobile and admin servers. By
default, five failed attempts for one source IP and login identifier within 15
minutes lock that login for 15 minutes; 20 failed attempts from one IP also lock
that source even when identifiers are rotated. Rate-limited responses use HTTP
`429` with `Retry-After`. New and reset passwords must contain 12–128
characters. Existing password hashes remain valid until the password is reset.
The defaults can be tuned with:

- `CLOUDLEDGER_LOGIN_MAX_FAILURES`
- `CLOUDLEDGER_LOGIN_MAX_FAILURES_PER_IP`
- `CLOUDLEDGER_LOGIN_WINDOW_SECONDS`
- `CLOUDLEDGER_LOGIN_LOCKOUT_SECONDS`

The limits use the direct TCP peer address. Deployments behind a reverse proxy
must enforce equivalent limits at the proxy because forwarded client IP headers
are intentionally not trusted by the application.

Cloudflare Turnstile protects both organization and platform login forms. Set
both variables from the Turnstile widget configured for the admin hostname:

- `CLOUDLEDGER_TURNSTILE_SITE_KEY`
- `CLOUDLEDGER_TURNSTILE_SECRET_KEY`

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
by default; use `CLOUDLEDGER_SERVER_DATA_DIR` to move that state elsewhere.

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
CLOUDLEDGER_ADMIN_BIND_ADDR=127.0.0.1:8788 nix develop path:. -c cargo run -p cloudledger-server
```

Read `.cloudledger-server/admin-path`, then open
`http://127.0.0.1:8788/<admin-path>`. Use the platform-token tab with
`.cloudledger-server/admin-token` to create organizations. Afterwards, each
organization administrator uses the organization-account tab to create and
manage that organization's employee accounts.

For the public ledger approval smoke tests:

1. Submit a public-ledger transaction from the logged-in phone account and
   confirm it remains pending until another eligible account decides it.
2. Create another member from the admin backend with an eligible role, log in
   as that member on a separate installation id or use service tests, then
   approve/reject and confirm transaction state, posted balance behavior, and
   audit actor are correct.
