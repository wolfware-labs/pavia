# Pavia

Pavia is a real-time RPC and messaging protocol between browsers or apps and servers, with a Rust server implementation and a generated TypeScript client. It keeps the SignalR connection model (one persistent session, calls in both directions, server-side targeting of sessions, users and groups) and adds typed contracts, multiplexed streaming calls, WebTransport with WebSocket fallback, datagrams, channels with history, and session resumption.

Status: specification stage. No code yet.

## Documents

- [Specification](spec/pavia-protocol.md), with the [versioning policy and version index](spec/README.md).
- [Roadmap](ROADMAP.md): milestones M0 to M18, capability IDs (`C4.7`), tests and exit criteria. Each capability is a GitHub issue; each milestone is a GitHub milestone.

## Working on the spec

Any edit to `spec/pavia-protocol.md` is a new spec version. See [spec/README.md](spec/README.md) before opening a PR; CI enforces the rules.
