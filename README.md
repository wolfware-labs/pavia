# Pavia

Pavia is a real-time RPC and messaging protocol between browsers or apps and servers, with a Rust server implementation and generated clients. One persistent session carries calls in both directions and lets the server target sessions, users and groups. On top of that:

- Typed contracts in both directions, described by a machine-readable manifest that client generators consume.
- Multiplexed calls in four shapes (unary, server stream, client stream, bidirectional) with cancellation, deadlines, metadata and structured errors.
- WebTransport over HTTP/3 as the preferred binding and WebSocket as the universal fallback, with one logical protocol over both.
- Datagrams for lossy, latest-value data such as cursors and telemetry.
- Channels with durable history, gapless recovery and presence.
- Session resumption across transport loss.

The wire format is binary CBOR framed so that routers and backplanes can read envelopes without decoding payloads, which lets a broadcast be encoded once per codec.

## Non-goals

- Compatibility with the SignalR, Socket.IO, gRPC or MQTT wire formats.
- SSE or long-polling transports.
- Client-to-client messaging without a server hop.
- Media delivery.

## Documents

- [Specification](spec/pavia-protocol.md). Every published version is kept, unchanged, under `spec/versions/`; see the [versioning policy and index](spec/README.md).
- [Roadmap](ROADMAP.md): milestones, capability IDs (`C4.7`), tests and exit criteria. Each capability is a GitHub issue and each milestone a GitHub milestone.
- [Design records](docs/design/): decisions that shape more than one issue.

## Conformance

The specification defines conformance levels (Core, Streaming, Server calls, WebTransport, Datagrams, Resumption, Channels, History, Presence) and a public suite of test vectors and scripts (spec section 17). An implementation states which levels it supports; the rest is negotiated per session through capabilities. The conformance matrix in the roadmap tracks which levels pass on which binding, codec and client.

## Working on the spec

Any edit to `spec/pavia-protocol.md` is a new spec version and lands together with the matching test vectors. See [spec/README.md](spec/README.md) before opening a PR; CI enforces the rules.
