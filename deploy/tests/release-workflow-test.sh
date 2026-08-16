#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
verify_script="$script_dir/../verify-release-source.sh"
next_alpha_script="$script_dir/../next-alpha-tag.sh"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/cloudledger-release-test.XXXXXX")"
trap 'rm -rf "$test_root"' EXIT

alpha_tag="$(printf '%s\n' \
  v0.1.8.alpha.1 \
  v0.1.8.alpha.3 \
  v0.1.7.alpha.99 \
  v0.1.8.alpha.invalid \
  v0.1.8 \
  | bash "$next_alpha_script" 0.1.8)"
[[ "$alpha_tag" == v0.1.8.alpha.4 ]] || {
  printf 'Unexpected next alpha tag: %s\n' "$alpha_tag" >&2
  exit 1
}
if printf '%s\n' v0.1.8.alpha.1 | bash "$next_alpha_script" invalid >/dev/null 2>&1; then
  printf 'An invalid alpha base version was accepted.\n' >&2
  exit 1
fi

git init --bare "$test_root/remote.git" >/dev/null
git init "$test_root/work" >/dev/null
git -C "$test_root/work" config user.email test@example.invalid
git -C "$test_root/work" config user.name 'CloudLedger release test'
printf 'release commit\n' >"$test_root/work/README"
git -C "$test_root/work" add README
git -C "$test_root/work" commit -m 'release fixture' >/dev/null
git -C "$test_root/work" branch -M main
git -C "$test_root/work" branch release/v9.9.9
git -C "$test_root/work" remote add origin "$test_root/remote.git"
git -C "$test_root/work" push origin main release/v9.9.9 >/dev/null

git clone "$test_root/remote.git" "$test_root/clone" >/dev/null
git -C "$test_root/clone" fetch --no-tags origin \
  '+refs/heads/*:refs/remotes/origin/*' >/dev/null
release_sha="$(git -C "$test_root/clone" rev-parse refs/remotes/origin/release/v9.9.9)"

pushd "$test_root/clone" >/dev/null
RELEASE_TAG=v9.9.9 RELEASE_SHA="$release_sha" \
  RELEASE_REMOTE=origin bash "$verify_script" >/dev/null

git -C "$test_root/work" checkout main >/dev/null
printf 'main-only commit\n' >>"$test_root/work/README"
git -C "$test_root/work" add README
git -C "$test_root/work" commit -m 'main fixture' >/dev/null
git -C "$test_root/work" push origin main >/dev/null
main_sha="$(git -C "$test_root/work" rev-parse HEAD)"

if RELEASE_TAG=v9.9.9 RELEASE_SHA="$main_sha" RELEASE_REMOTE=origin \
  bash "$verify_script" >/dev/null 2>&1; then
  printf 'A tag from main was incorrectly accepted.\n' >&2
  exit 1
fi

if RELEASE_TAG=alpha-123 RELEASE_SHA="$release_sha" RELEASE_REMOTE=origin \
  bash "$verify_script" >/dev/null 2>&1; then
  printf 'An alpha tag was incorrectly accepted as formal.\n' >&2
  exit 1
fi
popd >/dev/null

printf 'Release source policy tests passed.\n'
