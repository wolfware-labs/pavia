# pavia-cli

The `pavia` binary: frame decoder and encoder in diagnostic notation (built on the `wire` layer of `pavia-proto`), `contract diff` and `contract fetch` over manifest files and the well-known contract URL, development certificates, and the code generation entry point. Manifest export is not a CLI command: applications export through `pavia-server`, so the CLI never links application code.

Rules: Must not pull a runtime into `pavia-proto` or `pavia-contract`; it links them as libraries.

Part of the Pavia workspace; see the repository README for the layout and `docs/design/sans-io.md` for the crate boundaries.
