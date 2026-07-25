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
testing bind it to a specific private address, for example
`10.0.0.42:8788`; the server rejects `0.0.0.0` and public IPs for this admin
port. The platform token comes from `CLOUDLEDGER_ADMIN_TOKEN` or the server data
directory's `admin-token` file.

The `/admin` page has separate platform and organization entry points. The
platform token creates and lists organizations. Every organization is created
with its own organization-admin login, public ledger, and default company bank
account. Organization admins log in with their own email/phone and password and
can manage employees only inside their organization.

New organization-admin accounts are backend-only identities and do not receive
a personal ledger; `POST /auth/login` rejects them. Employee accounts use the
mobile/Web business frontend, belong to one organization only, and cannot log in
to the organization admin backend. Existing persisted `owner` or `admin`
membership accounts are migrated to backend-only organization admins when the
server starts.

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

Then open `http://127.0.0.1:8788/admin`. Use the platform-token tab with
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
