#!/usr/bin/env bash
# Enforces the spec versioning rules described in spec/README.md.
# Usage: scripts/spec-guard.sh <base-ref> <head-ref>
set -euo pipefail

base="${1:?base ref}"
head="${2:?head ref}"
draft="spec/pavia-protocol.md"
fail() { echo "spec-guard: $*" >&2; exit 1; }

# 1. Existing snapshots are immutable.
changed=$(git diff --name-only --diff-filter=MDRT "$base" "$head" -- spec/versions || true)
if [ -n "$changed" ]; then
  fail "files under spec/versions/ were modified, renamed or deleted:"$'\n'"$changed"
fi

# 2. A change to the editor's draft must come with a new, identical snapshot.
if git diff --quiet "$base" "$head" -- "$draft"; then
  echo "spec-guard: editor's draft unchanged, nothing to check"
  exit 0
fi

added=$(git diff --name-only --diff-filter=A "$base" "$head" -- 'spec/versions/*/pavia-protocol.md' || true)
count=$(printf '%s\n' "$added" | grep -c . || true)
[ "$count" -eq 1 ] || fail "the editor's draft changed but $count snapshot(s) were added under spec/versions/ (expected exactly 1)"

if ! git diff --quiet "$head:$draft" "$head:$added" 2>/dev/null; then
  git show "$head:$draft" > /tmp/spec-guard-draft
  git show "$head:$added" > /tmp/spec-guard-snap
  cmp -s /tmp/spec-guard-draft /tmp/spec-guard-snap || fail "$added is not byte-identical to $draft"
fi

# 3. The directory name must match the Version line of the document.
dir=$(basename "$(dirname "$added")")
version_line=$(git show "$head:$draft" | grep -m1 '^\*\*Version:\*\*' || true)
[ -n "$version_line" ] || fail "no '**Version:**' line found in $draft"
case "$dir" in
  draft-*)
    n="${dir#draft-}"
    echo "$version_line" | grep -q "draft $n" || fail "directory $dir does not match version line: $version_line"
    ;;
  *)
    echo "$version_line" | grep -q "\*\*Version:\*\* $dir\b" || fail "directory $dir does not match version line: $version_line"
    ;;
esac

# 4. The snapshot must be listed in the version index.
grep -q "versions/$dir/pavia-protocol.md" spec/README.md || fail "spec/README.md does not list $dir in the Versions table"

echo "spec-guard: ok ($added)"
