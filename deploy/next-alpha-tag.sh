#!/usr/bin/env bash
set -euo pipefail

base_version=${1:-}
if [[ ! "$base_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  printf 'Alpha base version must match MAJOR.MINOR.PATCH: %s\n' "$base_version" >&2
  exit 1
fi

escaped_base=${base_version//./\\.}
max_sequence=0
while IFS= read -r tag; do
  if [[ "$tag" =~ ^v${escaped_base}\.alpha\.([0-9]+)$ ]]; then
    sequence=$((10#${BASH_REMATCH[1]}))
    if (( sequence > max_sequence )); then
      max_sequence=$sequence
    fi
  fi
done

printf 'v%s.alpha.%d\n' "$base_version" "$((max_sequence + 1))"
