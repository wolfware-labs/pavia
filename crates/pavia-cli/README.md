# pavia-cli

The `pavia` binary: frame decoder and encoder in diagnostic notation, `contract export` and `contract diff`, development certificates, and the code generation entry point.

Rules: Must not pull a runtime into `pavia-proto` or `pavia-contract`; it links them as libraries.

Part of the Pavia workspace; see the repository README for the layout and `docs/design/sans-io.md` for the crate boundaries.
