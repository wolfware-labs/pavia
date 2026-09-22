# pavia-contract

The contract type system (spec section 12), manifest export and validation, the fingerprint, and the typed `cbor` and `json` mappings.

Rules: Must not depend on `pavia-proto` or on any IO crate. The typed layer sits on top of the untyped session, never inside it.

Part of the Pavia workspace; see the repository README for the layout and `docs/design/sans-io.md` for the crate boundaries.
