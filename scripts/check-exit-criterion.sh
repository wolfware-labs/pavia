#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
usage: scripts/check-exit-criterion.sh [patch]

Checks the M4 exit criterion: changing a Rust method signature and regenerating makes the
example app fail to typecheck at the call site.

  1. exports the example manifest from crates/pavia-server/tests/manifest.rs and generates
     the TypeScript module of clients/typescript/packages/example-chat from it;
  2. fails when either differs from its committed copy;
  3. typechecks the example app, which must pass;
  4. applies the patch (default scripts/exit-criterion/send-gains-a-parameter.patch), then
     exports and generates again;
  5. typechecks again, which must fail with exactly one error: TS2554 at the line of
     src/app.ts that calls chat.send.

Run it from a clean tree after `pnpm install` in clients/typescript. It refuses a dirty tree,
changes the manifest, the generated module and the files the patch touches while it runs, and
restores them on exit.
EOF
}

fail() {
  echo "check-exit-criterion: $*" >&2
  exit 1
}

case "${1:-}" in
  -h | --help)
    usage
    exit 0
    ;;
esac
[ "$#" -le 1 ] || {
  usage
  exit 2
}

patch=$(realpath "${1:-$(dirname "$0")/exit-criterion/send-gains-a-parameter.patch}")
cd "$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"

manifest=crates/pavia-server/tests/manifest/example.json
package=clients/typescript/packages/example-chat
module=$package/src/contract.ts
app=$package/src/app.ts

[ -z "$(git status --porcelain)" ] || fail "the tree has uncommitted changes; commit or stash them first"
git apply --check "$patch" || fail "$patch does not apply"

mapfile -t touched < <({
  printf '%s\n' crates/pavia-server/tests/manifest.rs "$manifest" "$module"
  git apply --numstat "$patch" | cut -f3-
} | sort -u)
saved=$(mktemp -d)
for file in "${touched[@]}"; do
  if [ -e "$file" ]; then
    mkdir -p "$saved/$(dirname "$file")"
    cp -p "$file" "$saved/$file"
  fi
done

restore() {
  for file in "${touched[@]}"; do
    if [ -e "$saved/$file" ]; then
      cp -p "$saved/$file" "$file"
    else
      rm -f "$file"
    fi
  done
  rm -rf "$saved"
}
trap restore EXIT

regenerate() {
  PAVIA_UPDATE_MANIFEST=1 cargo test --locked -q -p pavia-server --test manifest keeps_the_committed_manifest_current
  cargo run --locked -q -p pavia-cli -- codegen ts "$manifest" --out "$package/src"
}

typecheck() {
  pnpm --dir "$package" exec tsc -p tsconfig.json --noEmit --pretty false
}

echo "check-exit-criterion: export and generate from the committed contract"
regenerate
git diff --exit-code -- "$manifest" "$module" || fail "the committed manifest or generated module is out of date"

echo "check-exit-criterion: typecheck the example app"
typecheck || fail "the example app does not typecheck against the committed contract"

echo "check-exit-criterion: apply $patch, export and generate again"
git apply "$patch"
regenerate

calls=$(grep -n 'chat\.send(' "$app" | cut -d: -f1 || true)
[ "$(printf '%s\n' "$calls" | grep -c .)" -eq 1 ] || fail "expected one call of chat.send in $app, found lines: ${calls:-none}"

echo "check-exit-criterion: typecheck again, expecting TS2554 at ${app#"$package/"}:$calls"
if output=$(typecheck 2>&1); then
  fail "the example app still typechecks after the change"
fi
errors=$(printf '%s\n' "$output" | grep 'error TS' || true)
printf '%s\n' "$errors"
expected="^src/app\.ts\($calls,[0-9]+\): error TS2554: "
if [ "$(printf '%s\n' "$errors" | grep -c .)" -ne 1 ] || ! printf '%s\n' "$errors" | grep -qE "$expected"; then
  printf '%s\n' "$output" >&2
  fail "expected exactly one error, TS2554 at src/app.ts line $calls"
fi

echo "check-exit-criterion: ok"
