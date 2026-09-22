#!/usr/bin/env bash
# Verifies the crate dependency rules from docs/design/sans-io.md.
# Usage: scripts/check-deps.sh   (run from the repository root)
set -euo pipefail

fail=0
forbid() {
  local crate="$1"; shift
  local deps
  deps=$(cargo tree -p "$crate" -e normal --prefix none 2>/dev/null | awk '{print $1}' | sort -u)
  for banned in "$@"; do
    if grep -qx "$banned" <<<"$deps"; then
      echo "check-deps: $crate must not depend on $banned" >&2
      fail=1
    fi
  done
}

forbid pavia-proto        tokio hyper axum quinn h3
forbid pavia-contract     pavia-proto tokio hyper axum quinn
forbid pavia-webtransport pavia-proto
forbid pavia-codegen      pavia-proto pavia-server tokio
forbid pavia-server       axum
forbid pavia-client       pavia-server axum

if [ "$fail" -ne 0 ]; then exit 1; fi
echo "check-deps: ok"
