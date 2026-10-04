# Clients

One directory per client platform, each self-contained with its own tooling:

- `rust/`: the `pavia-client` crate, a member of the Cargo workspace at the repository root.
- `typescript/`: a pnpm workspace holding `@pavia/client`, the test-only `@pavia/vectors`, and later one package per framework adapter (`@pavia/react`, `@pavia/svelte`, `@pavia/vue`).

Further platforms get a sibling directory here. Every client is tested against the same vectors and conformance scripts under `vectors/`.
