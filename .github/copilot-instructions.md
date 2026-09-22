# Pavia: instructions for reviewers and agents

Pavia is a real-time RPC and messaging protocol (spec in `spec/pavia-protocol.md`) with a Rust server and generated clients. The spec and its test vectors are the oracle: there is no reference implementation to diff against.

## Layout

- `spec/`: editor's draft plus one frozen copy per version under `spec/versions/`. `spec/README.md` has the rules.
- `vectors/`: frame, codec and script vectors shared by every implementation.
- `crates/`: Rust workspace members (`pavia-proto` sans-IO core, `pavia-contract`, `pavia-contract-derive`, `pavia-server`, `pavia-axum`, `pavia-webtransport`, `pavia-codegen`, `pavia-cli`).
- `clients/rust/`: the `pavia-client` crate. `clients/typescript/`: pnpm workspace with `@pavia/client`.
- `docs/design/`: decision records. `ROADMAP.md`: milestones and capability IDs (`C4.7`) that issues and commits reference.

## Commands

- Rust: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `scripts/check-deps.sh` (crate dependency rules).
- TypeScript, from `clients/typescript`: `pnpm install`, `pnpm typecheck`, `pnpm lint`, `pnpm test`.
- Spec: `scripts/spec-guard.sh <base> <head>` (versioning rules).

## Rules every PR must keep

1. Spec first. A behavior change lands in `spec/pavia-protocol.md` and `vectors/` in the same PR as the code, or before it. Code that decides behavior the spec leaves open is a defect; flag it.
2. Spec versioning. Any edit to `spec/pavia-protocol.md` adds one byte-identical copy under `spec/versions/<version>/`, bumps the `**Version:**` line, adds a changelog entry and a row in `spec/README.md`. Files already under `spec/versions/` are never modified, renamed or deleted.
3. Crate boundaries. `pavia-proto` and `pavia-contract` have no IO, no async runtime, no `tokio`, `hyper`, `axum` or `quinn`; `pavia-contract` does not depend on `pavia-proto`; `pavia-webtransport` does not depend on `pavia-proto`; `pavia-client` does not depend on `pavia-server`; only `pavia-axum` exposes axum types. Both core crates carry `#![forbid(unsafe_code)]`.
4. Commits use Conventional Commits (`feat(proto): ...`, `docs(spec): ...`, `chore: ...`) and reference the capability ID or issue they implement. No trailers.
5. Writing style in docs, comments and issues: plain engineering prose, hyphens rather than dashes, no filler.

## Protocol invariants to check in code

- Every length, count and depth limit is checked before allocation; frames larger than `max_frame` are rejected on the Length field.
- Headers use deterministic CBOR with integer keys; unknown keys are ignored; tags in headers are rejected.
- Streams are independent: no code may assume ordering between a call's RESULT and frames on stream 0.
- `nx: true` on an ERROR means the handler never ran; nothing after dispatch may set it.
- Flow-control credit is granted when the application consumes an item, not when bytes arrive.
- PING, PONG and CLOSE bypass every queue and limit.
- Sequence numbers are per lane, per direction; `LANE_OPEN` is seq 1 on its lane.
- Credentials and resume tokens never appear in URLs, logs or tracing fields; resume tokens are single-use and compared in constant time.
- Rate limits and caps from spec section 16 are configurable and each has a test.

## What a good review comment looks like

Cite the spec section or the design record the code disagrees with. Prefer "section 6.4 says credit is granted on consumption; this grants on arrival" over style remarks. Do not ask for comments or docs that restate the code.
