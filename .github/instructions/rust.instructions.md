---
applyTo: "crates/**,clients/rust/**"
---

Reviewing Rust code:

- Check the crate against its README rules and `docs/design/sans-io.md`. `pavia-proto` exposes `handle_input` and `poll_output`; it never reads a clock, opens a socket or spawns a task. Time arrives as `Instant` in inputs.
- No `unsafe` in `pavia-proto` or `pavia-contract`.
- Decoders: bounded reads, limits checked before allocation, incremental decoding that tolerates arbitrary chunk boundaries, no panics on any input (fuzz targets exist for a reason).
- No locks held across `.await` in `pavia-server`; one writer task per transport.
- Errors carry the spec's close code or status code; internal error text is not sent to peers unless development mode is on.
- Tests: vectors from `vectors/` in both directions, table tests for state machines, scripts for session behavior. A new MUST without a test is a finding.
- Public items are documented; `cargo doc` produces no missing-docs warnings on the public crates.
