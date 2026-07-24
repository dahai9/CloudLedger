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
frontend checks `VITE_CLOUDLEDGER_CLOUD_URL`, defaulting to
`http://192.168.1.229:8787` for the current test network. Override it when the
developer machine IP changes.

The admin backend is intentionally separated from the mobile API. It listens on
`CLOUDLEDGER_ADMIN_BIND_ADDR`, defaulting to `127.0.0.1:8788`. For LAN admin
testing bind it to a specific private address, for example
`192.168.1.229:8788`; the server rejects `0.0.0.0` and public IPs for this
admin port. The admin token comes from `CLOUDLEDGER_ADMIN_TOKEN` or from the
server data directory's `admin-token` file.

Fresh server data starts uninitialized. After entering the admin token at
`/admin`, the setup wizard creates the single organization, the owner login
identity, the owner private ledger, the organization public ledger, and default
accounts. Current CloudLedger deployments are single-organization only; later
admin work manages members inside that one organization.

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
adb shell curl -sS --connect-timeout 5 http://192.168.1.229:8787/ready
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

Then open `http://127.0.0.1:8788/admin` from the development machine and enter
the admin token from `.cloudledger-server/admin-token`. On a new data directory,
complete the setup wizard before logging in from the Android app.

For the public ledger approval smoke tests:

1. Submit a public-ledger transaction from the logged-in phone account and
   confirm it remains pending until another eligible account decides it.
2. Create another member from the admin backend with an eligible role, log in
   as that member on a separate installation id or use service tests, then
   approve/reject and confirm transaction state, posted balance behavior, and
   audit actor are correct.
