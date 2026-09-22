Closes #

## What

## Checklist

- [ ] Commits follow Conventional Commits and reference the capability ID or issue.
- [ ] Behavior changes are in `spec/pavia-protocol.md` and `vectors/` in this PR (or already merged), with a new snapshot under `spec/versions/`.
- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `scripts/check-deps.sh` pass.
- [ ] `pnpm typecheck`, `pnpm lint`, `pnpm test` pass in `clients/typescript` (if touched).
- [ ] New MUSTs have a vector or script.
