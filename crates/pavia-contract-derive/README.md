# pavia-contract-derive

The `#[derive]` implementations for contract structs, enums and aliases. Lives in its own crate because proc macros must.

Rules: Depends on `syn`, `quote` and `proc-macro2` only; generates code against `pavia-contract`.

Part of the Pavia workspace; see the repository README for the layout and `docs/design/sans-io.md` for the crate boundaries.
