# pavia-webtransport

The HTTP/3 pieces the endpoint needs (control streams, SETTINGS, static QPACK, extended CONNECT, 405 for other requests) and the WebTransport session layer (stream and datagram tagging, capsules, draft negotiation), directly on quinn. Decision #120.

Rules: A transport, not protocol: must not depend on `pavia-proto`. `pavia-server` and `pavia-client` drive `pavia-proto` over it.

Part of the Pavia workspace; see the repository README for the layout and `docs/design/sans-io.md` for the crate boundaries.
