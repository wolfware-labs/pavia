# Clients

One directory per client platform, each self-contained with its own tooling:

- `rust/`: the `pavia-client` crate, a member of the Cargo workspace at the repository root.
- `typescript/`: a pnpm workspace holding `@pavia/client` and, later, framework adapters.

Further platforms get a sibling directory here. Every client is tested against the same vectors and conformance scripts under `vectors/`.
