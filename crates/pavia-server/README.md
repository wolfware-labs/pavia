# pavia-server

The tokio runtime around `pavia-proto`: one task per session, the session registry, targeting of sessions, users and groups, the broker boundary, lifecycle hooks and interceptors, and the export of the contract manifest from the registered methods.

Rules: Must not depend on axum. Web framework integration lives in `pavia-axum`.

Part of the Pavia workspace; see the repository README for the layout and `docs/design/sans-io.md` for the crate boundaries.
