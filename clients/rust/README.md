# pavia-client

A tokio client over `pavia-proto` with WebSocket and WebTransport drivers, typed through the same Rust definitions the server uses. It is also the raw test client and load generator of the conformance suite.

Rules: Must not depend on `pavia-server`.

Part of the Pavia workspace; see the repository README for the layout and `docs/design/sans-io.md` for the crate boundaries.
