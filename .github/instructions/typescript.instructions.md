---
applyTo: "clients/typescript/**"
---

Reviewing TypeScript:

- The runtime mirrors the Rust driver loop: framing, CBOR headers and state machines must produce the same bytes as `pavia-proto` for the same vectors.
- `i64` and `u64` are `bigint`; any path that turns them into `number` is a defect. `timestamp` is `Date`, `bytes` is `Uint8Array`.
- PONG is answered from the receive path, never from a timer; the client does not declare the server dead on a timer alone.
- `bufferedAmount` (or `WebSocketStream`) gates sends; producers pause above the high-water mark.
- Retries happen only when the error carries `nx: true`, the method is `idempotent`, or an idempotency key was attached.
- Generated code is never edited by hand; golden tests cover the generator output.
- Bundle size stays within the budget declared in the package.
