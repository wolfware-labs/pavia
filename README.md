# Pavia

Pavia is a real-time RPC and messaging protocol between browsers or apps and servers, with a Rust server implementation and generated clients. One persistent session carries calls in both directions and lets the server target sessions, users and groups. On top of that:

- Typed contracts in both directions, described by a machine-readable manifest that client generators consume.
- Multiplexed calls in four shapes (unary, server stream, client stream, bidirectional) with cancellation, deadlines, metadata and structured errors.
- WebTransport over HTTP/3 as the preferred binding and WebSocket as the universal fallback, with one logical protocol over both.
- Datagrams for lossy, latest-value data such as cursors and telemetry.
- Channels with durable history, gapless recovery and presence.
- Session resumption across transport loss.

The wire format is binary CBOR framed so that routers and backplanes can read envelopes without decoding payloads, which lets a broadcast be encoded once per codec.

## Features

Checked items are implemented and covered by the conformance suite; the rest is planned, in roadmap order.

Protocol

- [ ] Session over WebSocket: handshake, capabilities, heartbeat, close codes, rate limits
- [ ] Unary calls and notifications in both directions, deadlines, cancellation, structured errors
- [ ] Typed contracts: manifest export, fingerprint, `cbor` and `json` codecs, compatibility checker
- [ ] Streaming calls: server stream, client stream, bidirectional, with flow control on WebSocket
- [ ] Targeting: sessions, users, groups, lifecycle hooks, slow-consumer handling
- [ ] WebTransport over HTTP/3 with automatic WebSocket fallback
- [ ] Datagrams and notification lanes
- [ ] Session resumption across transport loss
- [ ] Authentication lifecycle, per-method and per-channel authorization, interceptors
- [ ] Channels with history, gapless recovery and snapshots
- [ ] Presence

Implementations

- [ ] Rust server with an axum adapter
- [ ] TypeScript client generated from the manifest, tested in Chromium, Firefox and WebKit
- [ ] Rust client
- [ ] Multi-node deployment over NATS with JetStream
- [ ] Metrics, tracing and OpenTelemetry
- [ ] Public conformance suite for third-party implementations

## Non-goals

- Compatibility with the wire formats of other real-time or RPC frameworks.
- SSE or long-polling transports.
- Client-to-client messaging without a server hop.
- Media delivery.

## Documents

- [Specification](spec/pavia-protocol.md). Every published version is kept, unchanged, under `spec/versions/`; see the [versioning policy and index](spec/README.md).
- [Roadmap](ROADMAP.md): milestones, capability IDs (`C4.7`), tests and exit criteria. Each capability is a GitHub issue and each milestone a GitHub milestone.
- [Design records](docs/design/): decisions that shape more than one issue, including the crate layout ([`sans-io.md`](docs/design/sans-io.md)) and the protocol decisions of drafts 0.8 and 0.9 ([`spec-review-2026-10.md`](docs/design/spec-review-2026-10.md)).
- [Test vectors](vectors/README.md): the frame, codec and script formats every implementation is checked against.
- [Contributing](CONTRIBUTING.md) and the [review guide](.github/REVIEWING.md): how issues, pull requests and reviews work.

## Conformance

The specification defines conformance levels by feature (Core, Streaming, Server calls, Auth lifecycle, Lanes, Datagrams, Resumption, Channels, History, Presence) and a public suite of test vectors and scripts (spec section 17). Bindings (WebSocket, WebTransport) and codecs (`cbor`, `json`) are separate axes: an implementation states which levels it supports on which bindings and codecs, and the rest is negotiated per session through capabilities. The conformance matrix in the roadmap tracks which levels pass on which binding, codec and client.

## Repository layout

```
spec/                 the specification: editor's draft and every frozen version
vectors/              test vectors and conformance scripts shared by all implementations
docs/design/          decision records that shape more than one issue
crates/               Rust workspace members
  pavia-proto/          sans-IO core: framing, streams, session state, sequencing
  pavia-contract/       type system, manifest, fingerprint, typed codecs
  pavia-contract-derive/ derive macros for contract types
  pavia-server/         tokio runtime: session tasks, registry, targeting, hooks
  pavia-axum/           axum adapter
  pavia-webtransport/   HTTP/3 subset and WebTransport session layer on quinn
  pavia-codegen/        client code generation from a manifest
  pavia-cli/            the `pavia` binary
  pavia-vectors/        test support: checks the files under vectors/ (not published)
clients/              one directory per client platform, each with its own tooling
  rust/                 the `pavia-client` crate (a workspace member)
  typescript/           pnpm workspace with `@pavia/client` and the test-only `@pavia/vectors`
examples/             runnable example applications (from M7)
scripts/              repository checks used by CI
```

The Rust workspace is defined at the root and includes `crates/*` and `clients/rust`. Crate boundaries and the rules each crate must keep are in `docs/design/sans-io.md` and in every crate's README; `scripts/check-deps.sh` verifies them.

## Working on the spec

Any edit to `spec/pavia-protocol.md` is a new spec version and lands together with the matching test vectors. See [spec/README.md](spec/README.md) before opening a PR; CI enforces the rules.

## License

Copyright (c) 2026 Wolfware LLC. Licensed under either of the Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE)) or the MIT license ([LICENSE-MIT](LICENSE-MIT)), at your option; see [NOTICE](NOTICE). Contributions are accepted under the same terms.
