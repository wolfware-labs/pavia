# @pavia/example-chat

A small chat app written against the TypeScript client that `pavia codegen ts` generates from the
example manifest. It is private, never published, and never connects to a server.

- `src/contract.ts` is generated from `crates/pavia-server/tests/manifest/example.json`, the manifest
  the Rust test `crates/pavia-server/tests/manifest.rs` exports. Do not edit it; the codegen test
  `crates/pavia-codegen/tests/example.rs` fails when it is out of date, and
  `PAVIA_UPDATE_GENERATED=1 cargo test -p pavia-codegen --test example` writes it again.
- `src/app.ts` is the application code: it sends messages, turns the server's typed refusal into a
  reason to show, sends the typing notification and answers the server's calls.
- `test/app.test.ts` tests it against stand-ins for the generated client.

## The exit-criterion job

The `exit-criterion` CI job runs `scripts/check-exit-criterion.sh`, which checks the M4 exit
criterion: changing a Rust method signature and regenerating produces a compile error at the call
site in this app. The script exports the manifest, generates `src/contract.ts` and typechecks the
app, which must pass. Then it applies `scripts/exit-criterion/send-gains-a-parameter.patch`, which
gives `chat/send` a third parameter in the Rust contract, exports and generates again, and
typechecks once more. The job passes only when that typecheck fails with one error, TS2554, at the
line of `src/app.ts` that calls `chat.send`.

To run it locally, from a clean tree with `pnpm install` done in `clients/typescript`:

```
bash scripts/check-exit-criterion.sh
```

It restores the files it changes when it exits.
