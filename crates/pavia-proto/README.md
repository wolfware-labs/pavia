# pavia-proto

Wire encoding and decoding, stream state machines, the session state machine and sequencing, exposed through the `handle_input` / `poll_output` loop described in `docs/design/sans-io.md`.

Rules: Must not depend on tokio, hyper, axum or quinn, or on any other crate that performs IO or owns a runtime. Everything here is driven by a caller that feeds bytes, commands and clock readings.

Part of the Pavia workspace; see the repository README for the layout and `docs/design/sans-io.md` for the crate boundaries.
