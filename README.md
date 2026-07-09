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

Run the development cloud server on the LAN:

```bash
nix develop path:. -c cargo run -p cloudledger-server
```

The mobile frontend checks `VITE_CLOUDLEDGER_CLOUD_URL`, defaulting to
`http://192.168.1.229:8787` for the current test network. Override it when the
developer machine IP changes. The server persists its development identity in
`.cloudledger-server/server-id` by default; use `CLOUDLEDGER_SERVER_DATA_DIR` to
move that state elsewhere.

## Android Smoke Test

With a phone connected through ADB:

```bash
adb shell curl -sS --connect-timeout 5 http://192.168.1.229:8787/ready
adb install -r src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk
adb shell am start -n com.cloudledger.app/.MainActivity
```

Verify that Alice sees `Alice 私账` and `Acme 公账`, Bob sees `Bob 私账` and
`Acme 公账`, and Bob does not see `Alice 私账`.
