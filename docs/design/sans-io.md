# Sans-IO core and crate layout

Decision record for roadmap C0.7 (#2). Status: accepted, 2026-09-21.

## Decisions

1. The protocol crate is sans-IO and exposes one driver loop: `handle_input(Input)` then `poll_output()` until it returns `Output::Timeout`.
2. The Rust workspace has six library crates plus the derive crate; dependency rules are enforced by Cargo and checked in CI.
3. The TypeScript generator is a Rust crate, driven from the `pavia` binary.

## Why sans-IO

Everything that parses bytes or holds protocol state lives in a crate with no sockets, no timers and no async runtime. Time is an input. The consequences:

- The server, the Rust client, the load generator and both conformance runners run the same code. A disagreement between client and server about the wire is a bug in one place.
- Fuzz targets and script tests drive the exact production code path. A timeout bug reproduces from a script because the clock is a parameter.
- Drivers are thin: a WebSocket driver and a WebTransport driver differ in how they move bytes, not in what the protocol does.

## Crates

```
pavia-proto            wire, streams, session state machine, sequencing      no tokio, hyper, axum, quinn
pavia-contract         type system, manifest, fingerprint, typed codecs      no pavia-proto, no IO
pavia-contract-derive  #[derive] for contract types (proc-macro)            depends on nothing else here
pavia-server           tokio session tasks, registry, targeting, hooks       no axum
pavia-axum             mounting, request context, tower coexistence         axum + pavia-server
pavia-client           tokio Rust client, WS and WT drivers                  no pavia-server
pavia-codegen          TypeScript generator over a manifest                 pavia-contract only
pavia-cli              `pavia` binary: decode, contract export/diff, dev-cert, codegen
```

Rules:

- `pavia-proto` and `pavia-contract` carry `#![forbid(unsafe_code)]`.
- CI asserts the graph: `cargo tree -p pavia-proto -e normal` contains no `tokio`, `hyper`, `axum` or `quinn`; `cargo tree -p pavia-contract` contains no `pavia-proto`; `cargo tree -p pavia-client` contains no `pavia-server`.
- One workspace version; all crates release together.
- `pavia-server` depends on `pavia-proto` and `pavia-contract`; the typed layer sits on top of the untyped session, never inside it.

Rejected: four crates (contract types inside proto; client inside server) because the CLI and the client would link the server, and splitting after 1.0 is a breaking reshuffle. One crate with feature flags because additive features cannot enforce "no runtime in the core".

## Layers inside `pavia-proto`

```
session   handshake, heartbeat, close, GOAWAY, capabilities, seq/ack and replay,
          calls, notifications, lanes, channels, datagrams, scheduling classes (spec 6.6)
stream    call stream state machine (6.3), stream IDs (6.1), credit (6.4), lane streams
wire      varints (5.1), frames (5.2), WS envelope (5.4), header CBOR (5.4.1), datagram encoding (11.3)
```

Each layer is usable alone: the `pavia` decoder uses `wire` only; the state-machine table tests use `stream` only.

## Driver interface

One type, `Session`, for both roles and both bindings. Sketch, not final signatures:

```rust
pub struct Session { /* role, binding, config, state, output queue */ }

pub enum Input<'a> {
    Bytes { stream: StreamId, data: &'a [u8], fin: bool, now: Instant },
    StreamReset { stream: StreamId, now: Instant },          // WebTransport RESET_STREAM / STOP_SENDING
    Datagram { data: &'a [u8], now: Instant },
    TransportLost { now: Instant },
    Timeout { now: Instant },
    Command(Command),                                         // from the application
}

pub enum Output {
    Transmit { stream: StreamId, data: Bytes, fin: bool },    // WS: one message; WT: bytes for that QUIC stream
    OpenStream { stream: StreamId, kind: StreamKind },        // WT only; WS streams are implicit
    ResetStream { stream: StreamId, code: u64 },
    StopSending { stream: StreamId, code: u64 },
    Datagram(Bytes),
    CloseTransport { code: CloseCode, reason: String },
    Event(Event),                                             // to the application
    Timeout(Instant),                                         // terminal: nothing more until then or new input
}

pub enum Command {
    OpenCall { .. }, SendItem { .. }, EndCall { .. }, Cancel { .. },
    Notify { .. }, Subscribe { .. }, SendDatagram { .. }, Close { .. },
    RefreshAuth { .. }, ..
}

pub enum Event {
    Established { .. }, Rejected { .. }, Resumed { .. },
    CallOpened { .. }, Item { .. }, CallEnded { .. },
    Notification { .. }, Publication { .. }, Presence { .. }, Datagram { .. },
    GoAway { .. }, Closed { .. }, TransportDetached, ..
}
```

Rules the driver must keep:

- Drain `poll_output` until `Timeout` after every input. A debug assertion fires if input arrives with outputs still queued.
- Deliver inputs for one stream in order. Streams are independent.
- Arm exactly one timer, for the last `Timeout(at)` seen. `Session` computes the earliest of heartbeat, idle, handshake, deadlines and ack cadence.
- On WebSocket, one `Transmit` is one binary message: the session already applied the `WsFrame` envelope, batched frames across streams, and respected the `2 x max_frame` cap. The driver never splits or merges.
- On WebTransport, `Transmit.stream` names a QUIC stream the driver opened on `OpenStream`; the session never writes to a stream before asking for it.

Ordering and priority (spec 6.6) are decided inside `Session` when outputs are queued, so the starvation test is a unit test over `poll_output` order.

## Backpressure

- Outbound: the session keeps bounded queues (stream 0 queue, per-stream credit, replay buffer). When a bound is hit it emits the spec-mandated behavior (drop datagrams, `UNSUBSCRIBED` code 2, `SLOW_CONSUMER`) rather than growing.
- Inbound: the driver decides when to stop reading a socket (spec 4.3). It tells the session with `Command::PauseIdle` / `ResumeIdle` so the idle timer follows the reader.

## Where multi-session concerns live

`pavia-proto` knows one session. Registry, users, groups, fan-out, broker, hooks and interceptors are `pavia-server`. Encode-once fan-out (spec 13.6) is a server concern that passes pre-encoded `Bytes` into each session through `Command::Notify { data: Bytes }`.

## Testing plan tied to this layout

- `wire`: vectors in both directions, byte-by-byte chunking, fuzz targets for varint, frame, header, datagram.
- `stream`: exhaustive table tests (C1.8, C1.9).
- `session`: script runner as an in-memory driver; the same scripts later run over real sockets in `pavia-server` and `pavia-client`.

## Follow-ups

- C0.2 lays out the workspace with these crates and the CI graph assertions.
- C0.4 defines the script format the in-memory driver consumes.
