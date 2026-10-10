# Reviewing Pavia pull requests

Pavia is a real-time RPC and messaging protocol (spec in `spec/pavia-protocol.md`) with a Rust server and generated clients. The spec and its test vectors are the oracle: there is no reference implementation to diff against. This file and the path guides in `.github/instructions/` are the review rules; the spec and the decision records in `docs/design/` are the authority behind them.

## Layout

- `spec/`: editor's draft plus one frozen copy per version under `spec/versions/`. `spec/README.md` has the rules.
- `vectors/`: frame, codec, varint and script vectors shared by every implementation; `vectors/README.md` defines the formats, `vectors/schema/` validates their structure, and `crates/pavia-vectors` and `clients/typescript/packages/vectors` (test only, unpublished) check their content.
- `crates/`: Rust workspace members (`pavia-proto` sans-IO core, `pavia-contract`, `pavia-contract-derive`, `pavia-server`, `pavia-axum`, `pavia-webtransport`, `pavia-codegen`, `pavia-cli`; `pavia-macros` is planned in #218).
- `clients/rust/`: the `pavia-client` crate. `clients/typescript/`: pnpm workspace with `@pavia/client`.
- `docs/design/`: decision records (`sans-io.md` for crates and layering, `spec-review-2026-10.md` for the draft 0.8 and 0.9 protocol decisions). `ROADMAP.md`: milestones and capability IDs (`C4.7`) that issues and commits reference.
- User guides live in the separate `wolfware-labs/pavia-docs` repository.

## Commands

- Rust: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `scripts/check-deps.sh` (crate dependency rules).
- TypeScript, from `clients/typescript`: `pnpm install`, `pnpm typecheck`, `pnpm lint`, `pnpm test`.
- Spec: `scripts/spec-guard.sh <base> <head>` (versioning rules).
- Vectors: `uvx check-jsonschema --schemafile vectors/schema/<kind>.schema.json vectors/<kind>/*.json`.

## Rules every PR must keep

1. Spec first. A behavior change lands in `spec/pavia-protocol.md` and `vectors/` in the same PR as the code, or before it. Code that decides behavior the spec leaves open is a defect; flag it.
2. Spec versioning. Any edit to `spec/pavia-protocol.md` adds one byte-identical copy under `spec/versions/<version>/`, bumps the `**Version:**` line, adds a changelog entry and a row in `spec/README.md`. Files already under `spec/versions/` are never modified, renamed or deleted.
3. Crate boundaries. `pavia-proto` and `pavia-contract` have no IO, no async runtime, no `tokio`, `hyper`, `axum` or `quinn`; `pavia-contract` does not depend on `pavia-proto`; `pavia-webtransport` does not depend on `pavia-proto`; `pavia-client` does not depend on `pavia-server`; only `pavia-axum` exposes axum types; `pavia-contract-derive` generates code against `pavia-contract` only, and server-side macros belong in `pavia-macros`. Both core crates carry `#![forbid(unsafe_code)]`.
4. The `pavia` CLI works on manifest files and URLs; it never links application code (#67).
5. Commits use Conventional Commits (`feat(proto): ...`, `docs(spec): ...`, `chore: ...`) and reference the capability ID or issue they implement.

## Protocol invariants to check in code

Limits and decoding

- Every length, count and depth limit is checked before allocation; a frame larger than `max_frame` is rejected on its Length field (5.2).
- `max_frame` and `window` are at least 64 KiB on both peers; a client pipelines at most 64 KiB before WELCOME (4.1, 16).
- Headers use deterministic CBOR with integer keys; unknown keys are ignored; tags, indefinite lengths, oversize and excess depth in headers are PROTOCOL_ERROR (5.4.1).
- Data sections have one encoding per value: struct fields in manifest declaration order, `map` keys sorted, shortest integer and float forms, compact `json` with RFC 8785 strings and numbers. Decoders accept any order (12.3).
- An undecodable data section outside CALL and NOTIFY has the per-call or per-message outcome of 8.10, not a session close.

Handshake and session

- The handshake timeout runs from transport accept until WELCOME or REJECT is sent, covering authentication and the on-connect hook (7.1).
- Group joins made by the on-connect hook take effect when WELCOME is queued; nothing but WELCOME, REJECT or CLOSE is sent before WELCOME (4.1, 13.4).
- PING, PONG and CLOSE bypass every queue and limit; PINGs are rate-limited by a token bucket that any other frame refills (7.4).
- One WebTransport session per QUIC connection (4.2).

Calls and streams

- Streams are independent: no code may assume ordering between a call's RESULT and frames on stream 0.
- An early RESULT or ERROR from the callee completes the call and frees the stream; late frames are handled as frames on a closed stream (6.3). A FIN before the terminal frame is handled like a reset. On WebTransport a caller that already finished sending cancels with STOP_SENDING (8.5).
- Flow-control credit is granted when the application consumes an item, not when bytes arrive (6.4).
- `nx: true` on an ERROR means the handler never ran; nothing after dispatch may set it.
- An idempotency key only makes a call retryable when the `idempotency` capability was granted (8.7).

Resumption

- Sequence numbers are per lane, per direction, and assigned when a frame is first written to a transport. LANE_OPEN is not sequenced (7.8, 9.2).
- Control requests, their replies and DGRAM_BIND are never evicted from the replay buffer; every other eviction of a frame that cannot be recovered by resubscribing is reported in `lost` (7.8).
- Credentials and resume tokens never appear in URLs, logs or tracing fields; resume tokens are single-use, compared in constant time, and spent only by a successful resume. The `resume` map carries the session ID (key 19) for lookup.

Limits and abuse

- Rate limits are token buckets; every limit in section 16 is configurable and has a test (14, 16).

## Architecture to check in code

Report a structural problem as a finding, the same as a defect.

- Layering. Inside `pavia-proto` the layers of `docs/design/sans-io.md` depend downward only: `session` may use `stream` and `wire`, `stream` may use `wire`, never the reverse. Within `wire`, the frame codec uses varints and header sections, the WebSocket envelope uses varints and frames, and the datagram encoding (11.3) uses varints; no other `wire` module depends on another.
- One spec concept per module, for example varints (5.1), frames (5.2), header sections (5.4.1). A file that mixes two is a finding.
- Minimal surface. Items are private or `pub(crate)` unless another crate needs them. An item another crate can name (`pub`, reachable through public modules or re-exports) is public API; the PR description names the caller that needs it. A `pub` item inside a private module is crate-internal and is not a finding.
- Format apart from policy. Encoding and decoding follow the format; the defaults of spec section 16 (sizes, depths, timeouts, counts) arrive as parameters or configuration, not as constants inside a codec.
- Errors belong to their layer. A module returns its own error type. Mapping errors to close codes or call status codes happens only in the error model (C1.12); until it lands, nothing maps them.
- APIs shaped for their caller. Check a new function against the issues it blocks (their "Blocked by" relationships): an API the caller has to work around, or one that allocates per varint, header field or frame, is a finding.
- No speculative structure. A trait, generic parameter or extension point with a single user needs a reason in the design records or the issue.

## What a good review comment looks like

Cite the spec section or the design record the code disagrees with. Prefer "section 6.4 says credit is granted on consumption; this grants on arrival" over style remarks. Report only what would change the code, the spec text or the vectors.
