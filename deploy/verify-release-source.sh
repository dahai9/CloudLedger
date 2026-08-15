#!/usr/bin/env bash
set -euo pipefail

release_tag="${RELEASE_TAG:-${GITHUB_REF_NAME:-}}"
release_sha="${RELEASE_SHA:-${GITHUB_SHA:-}}"
remote="${RELEASE_REMOTE:-origin}"

if [[ ! "$release_tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  printf 'Formal release tags must match vMAJOR.MINOR.PATCH: %s\n' "$release_tag" >&2
  exit 1
fi

if [[ ! "$release_sha" =~ ^[0-9a-fA-F]{40}$ ]]; then
  printf 'Release commit must be a full Git SHA: %s\n' "$release_sha" >&2
  exit 1
fi

release_branch="release/$release_tag"
release_ref="refs/remotes/$remote/$release_branch"

# A formal tag must point at the current tip of its matching release branch.
# This keeps tags created from main (or another branch) out of the stable path.
git fetch --no-tags "$remote" \
  "refs/heads/$release_branch:$release_ref" >/dev/null
branch_sha="$(git rev-parse --verify "$release_ref^{commit}")"

if [[ "$branch_sha" != "$release_sha" ]]; then
  printf 'Tag %s is not the tip of %s (tag=%s branch=%s).\n' \
    "$release_tag" "$release_branch" "$release_sha" "$branch_sha" >&2
  exit 1
fi

printf 'Verified %s points to %s at %s.\n' "$release_tag" "$release_branch" "$release_sha"
