set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

flake := "path:."
tauri_dir := env_var_or_default("TAURI_DIR", ".")

default:
    just --list

check:
    nix develop {{flake}} -c cargo fmt --all -- --check
    nix develop {{flake}} -c cargo clippy --workspace --all-targets -- -D warnings
    nix develop {{flake}} -c cargo test --workspace
    nix develop {{flake}} -c npm install
    nix develop {{flake}} -c npm run build

# Enter the pinned development shell.
shell:
    nix develop {{flake}}

# Verify the toolchain that this scaffold is responsible for.
doctor:
    nix develop {{flake}} -c bash -lc 'rustc --version && cargo --version && node --version && npm --version && java -version && adb version && test -n "$ANDROID_HOME" && test -n "$NDK_HOME" && echo "ANDROID_HOME=$ANDROID_HOME" && echo "NDK_HOME=$NDK_HOME"'

# Install JS workspace dependencies after package owners add their packages.
workspace-install:
    nix develop {{flake}} -c npm install

# Rust recipes become meaningful after Rust/Tauri owners add workspace members.
fmt:
    nix develop {{flake}} -c cargo fmt --all

check-rust:
    nix develop {{flake}} -c cargo check --workspace --all-targets

lint-rust:
    nix develop {{flake}} -c cargo clippy --workspace --all-targets -- -D warnings

# Tauri recipes expect the app package at TAURI_DIR, defaulting to the repo root.
tauri-dev:
    nix develop {{flake}} -c bash -lc 'cd "{{tauri_dir}}" && npm run tauri:dev'

android-init:
    nix develop {{flake}} -c bash -lc 'cd "{{tauri_dir}}" && npm run tauri:android:init'

android-dev:
    nix develop {{flake}} -c bash -lc 'cd "{{tauri_dir}}" && npm run tauri:android:dev'

android-build:
    nix develop {{flake}} -c bash -lc 'cd "{{tauri_dir}}" && npm run tauri:android:build'

test:
    nix develop {{flake}} -c cargo test --workspace

frontend-build:
    nix develop {{flake}} -c npm run build

server:
    nix develop {{flake}} -c cargo run -p cloudledger-server
