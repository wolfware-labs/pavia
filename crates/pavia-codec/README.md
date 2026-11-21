# pavia-codec

Typed data sections: a value of a contract type (spec section 12.1) encoded and decoded in the `cbor` and `json` codecs exactly as spec section 12.3 maps them, on top of the untyped data-section codec of `pavia-proto`.

Rules: Must not perform IO. It depends on `pavia-proto` and `pavia-contract` and sits under `pavia-server` and `pavia-client`; `pavia-contract` stays free of `pavia-proto`, so code generation and the CLI never link the protocol crate.

Part of the Pavia workspace; see the repository README for the layout and `docs/design/sans-io.md` for the crate boundaries.
