# Contributing

Pavia is developed spec first. Read `spec/pavia-protocol.md` and `ROADMAP.md` before opening anything.

## Issues

Use the issue forms: a capability from the roadmap, a spec finding, or a bug. Capability issues are the unit of work; each has a roadmap ID like `C4.7` and belongs to a milestone.

## Pull requests

- One capability or one spec change per PR, referencing its issue.
- A behavior change lands in the spec and in `vectors/` with the code. Editing `spec/pavia-protocol.md` means cutting a new version; `spec/README.md` explains how and CI checks it.
- Commits follow Conventional Commits and are signed.
- Before pushing: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `scripts/check-deps.sh`; in `clients/typescript`: `pnpm typecheck`, `pnpm lint`, `pnpm test`.

## Design decisions

Decisions that shape more than one issue are recorded under `docs/design/` and linked from the issues they affect. Decisions local to one issue are recorded in that issue.

## License

Contributions are accepted under MIT OR Apache-2.0, the project's license.
