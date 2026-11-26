# pavia-contract

The contract type system (spec section 12), manifest export, reading and validation, and the fingerprint. The typed `cbor` and `json` mappings live in `pavia-codec`.

Rules: Must not depend on `pavia-proto` or on any IO crate. The typed layer sits on top of the untyped session, never inside it.

Part of the Pavia workspace; see the repository README for the layout and `docs/design/sans-io.md` for the crate boundaries.
