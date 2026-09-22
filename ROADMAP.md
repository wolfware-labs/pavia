# Pavia Implementation Roadmap

> Companion to **`spec/pavia-protocol.md`**. The spec defines *what* the protocol is; this roadmap orders the work, states what the implementation must be able to do at each step, and how to prove it. Architecture and code design are deliberately left to you.

---

## How to use this document

- Work milestones **in order**; each builds on the previous ones.
- Every capability has an ID (`C4.7`) for tests, issues and commits. Spec references look like `§8.4`.
- A milestone is **done** only when its *Exit criteria* pass in CI.
- **Spec-first rule:** there is no reference implementation to diff against, so the spec and its test vectors are the oracle. Any behavior change updates `spec/pavia-protocol.md` and `vectors/` **in the same change**, before or with the code. If you find yourself deciding behavior in code, stop and write it into the spec first.
- **`[verify]`** marks third-party behavior (browsers, drafts, libraries) that must be confirmed empirically before relying on it.

### Release lines

| Line | Milestones | Meaning |
|---|---|---|
| **MVP** | M0-M7 | WebSocket binding, typed unary/streaming calls both directions, generated TypeScript client, targeting, axum integration. Usable, not announced. |
| **Headline 0.x** | M8-M9 | WebTransport binding, transport racing, datagrams, lanes. **Announce here.** |
| **Feature complete** | M10-M15 | Resumption, auth lifecycle, channels with history, presence, client extras, multi-node. |
| **1.0** | M16-M18 | Hardening, ergonomics, spec freeze, public conformance suite. |

---

## Testing strategy

| Layer | What | From |
|---|---|---|
| **1. Vectors** | Frame, codec and invalid-input vectors in `vectors/`, hand-reviewed against the spec. Every parser and encoder runs against them in both directions. | M1 |
| **2. Property + fuzz** | `proptest` round-trips for every frame type and contract type; `cargo-fuzz` targets for varint, frame, CBOR header, codec decoders, WS envelope, datagram decoder. | M1 |
| **3. Script conformance** | Scenario scripts (`send` / `expect` / `expect_nothing_within` / `close_transport` / `wait`) per spec §17.2. A **server runner** plays the client side against your server over a real socket. A **client runner** (fake server) plays the server side against a client under test. | M2 (server), M4 (client) |
| **4. Browser end-to-end** | Generated TS client in real browsers via Playwright: Chromium, Firefox, WebKit. Same scenarios as layer 3, at the API level. | M4 |
| **5. Binding matrix** | Every layer-3/4 scenario runs on WebSocket **and** WebTransport; results must be identical except for documented binding differences (spec §4.5). | M8 |
| **6. Abuse suite** | Scripts that trigger every limit in spec §16 (rapid reset, ping flood, handshake flood, slow HELLO, varint/CBOR bombs, datagram floods) and assert the specified close code within a bounded time. Grows with every milestone that adds a limit. | M2 |
| **7. Fault + soak** | TCP faults via toxiproxy; UDP/QUIC faults via `tc netem` or a UDP proxy (loss, reorder, blackhole to simulate UDP-blocked networks); long runs with churn to catch leaks. | M5 (soak), M8 (UDP) |

CI runs layers 1-6 on every PR once they exist; layer 7 nightly.

---

## M0: Groundwork

**Goal:** own the spec, set up the repositories and the test oracles before any server logic.

**Spec:** read it end to end; file every disagreement as an issue before starting.

### Deliverables
- [ ] **C0.1 Spec review pass.** Read the whole spec; resolve or explicitly defer every item in §18 that affects M1-M7 (at least #2 method interning, #7 WS session flow control). Record decisions in the spec.
- [ ] **C0.7 Minimal Rust client from day one.** Plan the sans-IO crate so a Rust client (used as the raw test client, the load generator and later the real client of M14) is the same code as the server's protocol side. It appears in M2 as the script runner and grows with each milestone.
- [ ] **C0.2 Repository layout.** Rust workspace and a TypeScript workspace (monorepo or two repos; decide). At minimum: a sans-IO protocol crate, a server crate, an axum adapter crate, a TS runtime package, and a codegen entry point (cargo subcommand or standalone binary).
- [ ] **C0.3 CI.** Rust: fmt, clippy `-D warnings`, tests. TS: typecheck, lint, tests. Both on every PR.
- [ ] **C0.4 Vector format.** Define the file format for frame vectors, codec vectors and scripts (spec §17.2), with a README. Write 5 hand-made vectors to validate the format.
- [ ] **C0.5 Diagnostic tooling.** A small CLI that decodes hex/binary frames to diagnostic notation and back. You'll use it constantly to author vectors and debug.
- [ ] **C0.6 Scope statement.** README section: what Pavia is, conformance levels targeted for MVP, explicit non-goals.

### Exit criteria
- CI green on empty crates/packages.
- Vector format documented, with a working decoder CLI round-tripping the sample vectors.

**Rust focus:** workspace organization, CLI with `clap`, CBOR diagnostic handling.

---

## M1: Wire core (sans-IO)

**Goal:** a pure library that encodes and decodes every frame in the spec. No tokio, no sockets, no HTTP.

**Spec:** §5, §6.1, §6.3 (as state rules), §15.

### Capabilities
- [ ] **C1.1 Varints** (§5.1): encode shortest form; decode any valid form; reject values above caller-supplied limits before allocating.
- [ ] **C1.2 Frame codec** (§5.2): Type, Flags, Length, HeaderLength, Header, Data. Incremental decoding: accepts arbitrary byte chunks, yields complete frames, keeps remainders.
- [ ] **C1.3 WS envelope** (§5.4): StreamId prefix; multiple frames per message; reject partial trailing frames.
- [ ] **C1.4 Header sections** (§5.4.1): deterministic CBOR encoding with integer keys; decoding that rejects indefinite lengths, enforces size and depth limits, and ignores unknown keys.
- [ ] **C1.5 Typed frame model** for every frame type in the registry (§5.6) with every header field in §5.7, required/optional per the spec. Unknown must-understand types → error; extension types (0x80-0xFF) → skipped.
- [ ] **C1.6 Flags:** emit 0; ignore unknown bits on receive.
- [ ] **C1.7 Direction and stream-kind validation** (§5.6): a helper that reports whether a frame is allowed on a given stream kind (control / call / lane / datagram) from a given side.
- [ ] **C1.8 Call stream state machine** (§6.3) as pure state: given shape, role and a frame, return next state or a protocol error. Covers all four shapes, terminal frames, early callee termination, CANCEL, frames after terminal.
- [ ] **C1.9 WS stream ID rules** (§6.1): parity per initiator, strictly increasing, closed-ID tolerance, never-opened-ID error.
- [ ] **C1.10 Dynamic codec values:** encode/decode data sections for `cbor` and `json` as untyped values (typed mapping comes in M4). JSON: UTF-8 validation.
- [ ] **C1.11 WT datagram encoding** (§11.3) and DATAGRAM/DGRAM_BIND frames, even though they're used much later.
- [ ] **C1.12 Error model** that distinguishes: session-fatal (PROTOCOL_ERROR, FRAME_TOO_LARGE), ignorable, and per-call errors.
- [ ] **C1.13 Sequencing model** (§7.8): per-lane, per-direction counters and `ack` maps as pure data structures with a bounded replay buffer, testable without IO. Used from M10 but designed now so frame types carry `seq`/`ack` from the start.
- [ ] **C1.14 Tag and depth strictness** (§12.3, §14): headers reject tags; typed data accepts only declared tags; depth limits on data sections.

### Tests
- **Vectors:** at least one valid vector per frame type, per binding; invalid vectors for every "MUST reject" in §5 (indefinite-length header, oversized varint, text in type slot, truncated frame, bad direction, etc.).
- **Incremental decoding:** every vector fed byte-by-byte and in random chunk sizes.
- **State machine:** exhaustive table tests for C1.8 (every shape × role × frame type × state).
- **Property:** `decode(encode(frame)) == frame` for generated frames.
- **Fuzz:** each decoder runs 10+ minutes clean.

### Exit criteria
- All vectors pass; fuzz clean; the crate has no async runtime or HTTP dependency.

**Rust focus:** `bytes::Buf`/`BufMut`, zero-copy slicing of data sections, enums and exhaustive matching, `proptest`, `cargo-fuzz`, `ciborium`/`minicbor` trade-offs (deterministic encoding support matters).

---

## M2: Session over WebSocket

**Goal:** a client can connect over WebSocket, complete HELLO/WELCOME, stay alive, and be closed correctly with every close code. No calls yet.

**Spec:** §4.1, §4.3, §6.2, §7.1-7.4, §7.6 (basic), §7.7, §7.10, §15.1.

### Capabilities
- [ ] **C2.1 Endpoint** accepts WebSocket upgrades on the configured path with subprotocol `pavia.1`; refuses upgrades without it.
- [ ] **C2.2 Origin validation** against an allow-list; loopback exemption for development.
- [ ] **C2.3 Binary-only:** a text message → CLOSE PROTOCOL_ERROR.
- [ ] **C2.4 HELLO parsing and handshake timeout** (default 10 s → HANDSHAKE_TIMEOUT).
- [ ] **C2.5 Version and codec negotiation;** no overlap → REJECT UNSUPPORTED_VERSION.
- [ ] **C2.6 Capability negotiation:** grant the subset implemented so far; reject later use of ungranted capabilities with PROTOCOL_ERROR.
- [ ] **C2.7 Authentication hook:** HELLO `auth` (and upgrade-request cookies) passed to a user-supplied authenticator; failure → REJECT UNAUTHENTICATED/FORBIDDEN; success → principal attached to the session.
- [ ] **C2.8 WELCOME** with session ID (public, unique), limits, `hb`, `idle`, `window`, granted caps, optional `user`.
- [ ] **C2.9 Pre-handshake discipline:** only HELLO accepted before WELCOME; nothing but WELCOME/REJECT/CLOSE sent.
- [ ] **C2.10 Heartbeat:** PING after `hb` ms of send silence; PONG echo (≤8 bytes) always answered, even under load.
- [ ] **C2.11 Idle timeout:** nothing received for `idle` → CLOSE IDLE_TIMEOUT.
- [ ] **C2.12 Frame size limit:** frame > `max_frame` → CLOSE FRAME_TOO_LARGE, checked before buffering the payload.
- [ ] **C2.13 CLOSE handling** both directions with WS close code `4000 + code`; peer-initiated WS close without CLOSE treated as transport loss (session ends, since resumption isn't implemented yet).
- [ ] **C2.14 Session registry:** sessions are registered after WELCOME and removed exactly once on end.
- [ ] **C2.15 Contract fingerprint exchange** plumbing (policy `warn` only for now; manifests arrive in M4).
- [ ] **C2.16 Server-at-capacity:** max sessions limit → REJECT LIMIT_EXCEEDED with `retry`.
- [ ] **C2.17 Pre-WELCOME pipelining** (§4.1): frames after HELLO are buffered up to the pre-auth limit, processed in order after WELCOME, discarded on REJECT; LIMIT_EXCEEDED when the buffer overflows.
- [ ] **C2.18 0-RTT:** early data is never processed (disable 0-RTT on the TLS/QUIC listener or defer reads); `permessage-deflate` is never negotiated; `426` for plain HTTP on the path.
- [ ] **C2.19 Inbound backpressure** (§4.3): stop reading above the unprocessed-bytes limit and pause the idle timer; resume within the grace period or SLOW_CONSUMER.
- [ ] **C2.20 Address-level rate limits:** handshakes per source address; PING rate limit per session (§7.4).
- [ ] **C2.21 Session ID** format and uniqueness (§7.2), generated so it stays unique across nodes without coordination.

### Tests
- **Script conformance (server runner):** one script per capability, including negative cases (text frame, no subprotocol, HELLO timeout, oversized frame, bad Origin, unknown must-understand frame, extension frame ignored).
- **Timing:** heartbeat and idle with short configured intervals.
- **Leak:** 10,000 connect/close cycles leave the registry empty.
- **Abuse suite (first entries):** handshake flood, slow-loris HELLO (one byte per second), oversized pre-auth pipeline, ping flood, text frame, 0-RTT HELLO (if the stack allows sending it); each ends with the specified code within a bounded time.

### Exit criteria
- All M2 scripts pass; the raw test client can hold a session idle for >3× `hb` without being closed.

**Rust focus:** per-session tasks, `tokio::select!` over reader/writer/timers, cancellation safety, a single writer task per transport to preserve frame integrity.

---

## M3: Calls and notifications (unary)

**Goal:** typed-agnostic RPC works both ways: clients call server methods and send notifications; the server calls client methods and sends notifications to the caller.

**Spec:** §6.3 (unary), §6.5, §8 (except streaming shapes), §9.1, §15.2.

### Capabilities
- [ ] **C3.1 Method registry** keyed by `"service/method"`, case-sensitive, separate namespaces for server and client methods (§8.1). Hand-written registration is fine; macros come in M17.
- [ ] **C3.2 Unary calls** (client → server): CALL → handler → RESULT; argument array decoding into handler parameters.
- [ ] **C3.3 Callee validation order** (§8.2): UNIMPLEMENTED, INVALID_ARGUMENT (shape), INVALID_ARGUMENT (args), then authorization (hook placeholder), RESOURCE_EXHAUSTED.
- [ ] **C3.4 Structured errors** (§8.4): status code + message + optional typed detail; unexpected failures and panics → INTERNAL with a generic message unless development mode is on.
- [ ] **C3.5 Panic isolation:** a panicking handler produces ERROR INTERNAL; session and server survive.
- [ ] **C3.6 Concurrency:** calls on different streams run concurrently; `max_calls` enforced (§6.5) with RESOURCE_EXHAUSTED.
- [ ] **C3.7 Deadlines** (§8.6): server-side timer → handler cancelled → DEADLINE_EXCEEDED; optional server default/max deadline.
- [ ] **C3.8 CANCEL for unary** (§8.5): handler cancelled; ERROR CANCELLED unless terminal already sent.
- [ ] **C3.9 Metadata** (§8.7): exposed to handlers; `traceparent` propagated into tracing spans.
- [ ] **C3.10 Notifications client → server** (§9.1): dispatched, never answered, errors only logged.
- [ ] **C3.11 Server → caller notifications** from inside a handler (NOTIFY on stream 0).
- [ ] **C3.12 Server-initiated unary calls** (§8.8, capability `server-calls`): server opens an odd stream ID, awaits RESULT/ERROR; respects the client's `max_calls`; failure paths (client UNIMPLEMENTED, session ends mid-call → local UNAVAILABLE, deadline).
- [ ] **C3.13 Ordering non-guarantee documented** (§8.9) in the server docs.
- [ ] **C3.14 `json` codec** end to end for debugging (session negotiates `json`, every data section is JSON).
- [ ] **C3.15 `nx` flag** (§8.4): set on every pre-dispatch failure and on UNAVAILABLE after GOAWAY; never set once a handler has started.
- [ ] **C3.16 CANCEL race** (§8.5): CANCEL after a terminal frame is ignored; duplicate CANCEL ignored.
- [ ] **C3.17 Stream open/CANCEL rate limit** (§6.5) → LIMIT_EXCEEDED; notification rate limit (§16).
- [ ] **C3.18 Idempotency keys** (§8.7, capability `idempotency`): pluggable outcome store with an in-memory implementation; same key + same principal returns the stored outcome; retention window.

### Tests
- **Scripts:** every status code path with the expected `nx` value; deadline; cancel; cancel racing a RESULT; panic; concurrency limit; rapid-reset (CALL+CANCEL loop → LIMIT_EXCEEDED); idempotency replay returns the identical outcome without re-executing (counter check); notification with a bad method (assert *nothing* comes back within a timeout); server-initiated call with and without a client handler (the runner plays the client).
- **Codec parity:** the same scripts run under `cbor` and `json`.

### Exit criteria
- All call/notify scripts pass under both codecs.

**Rust focus:** type-erased async handlers, cancellation via dropping futures or `CancellationToken`, `catch_unwind` on futures, pending-call maps keyed by stream ID with `oneshot` completion.

---

## M4: Contract, codegen and the TypeScript client (WebSocket)

**Goal:** the defining feature of the project. Rust definitions produce a manifest; the manifest produces a typed TypeScript client; that client works in real browsers.

**Spec:** §12 (all), §4.4 (WebSocket-only path for now), §5.5.

### Capabilities: Rust side
- [ ] **C4.1 Type system mapping** (§12.1): every contract type derivable from Rust types (a trait implemented for primitives, std collections, and via derive for user structs/enums). Unsupported Rust shapes are compile-time errors, not runtime surprises.
- [ ] **C4.2 Typed codec mapping** (§12.3) for both `cbor` and `json`, including the tricky rows: `i64`/`u64` as JSON strings, timestamps (CBOR tag 1 float / RFC 3339), UUID (tag 37 / string), bytes (base64url in JSON), enums externally tagged, optional fields omitted.
- [ ] **C4.3 Manifest export** (§12.2) from registered services, client methods, (declared-but-unused) channels and datagram topics. Deterministic output.
- [ ] **C4.4 Fingerprint** (§12.5): SHA-256 over RFC 8785 canonical JSON; sent in WELCOME; `warn`/`strict` policy (§7.2).
- [ ] **C4.5 Well-known endpoint** (§12.6), disabled by a config flag.
- [ ] **C4.6 Unknown struct fields ignored; closed enums reject unknown variants; open enums map them to the catch-all** (§12.1).
- [ ] **C4.6a Fingerprint scope** (§12.5): `docs` and `name` stripped before canonicalization; a docs-only change must not change the fingerprint (test it).
- [ ] **C4.6b Contract compatibility checker:** `pavia contract diff old.json new.json` implementing §12.7, exit code non-zero on breaking changes, meant for CI (the `buf breaking` idea). Ship it with M4 because contract evolution is where users get burned first.

### Capabilities: generator
- [ ] **C4.7 TypeScript generation** (§12.4): types, service proxies (unary + notify now; streaming signatures stubbed until M5), client method handler registration, typed errors with detail, fingerprint constant.
- [ ] **C4.8 Name mapping:** wire names preserved; any idiomatic renaming is reversible.
- [ ] **C4.9 Golden generator tests:** a fixed manifest produces byte-identical TS output (snapshot tests).

### Capabilities: TypeScript runtime
- [ ] **C4.10 WebSocket binding** (§4.3): framing, stream IDs, handshake, heartbeat, idle detection, close codes.
- [ ] **C4.11 Call API:** promises, `AbortSignal` → CANCEL, client-side deadline enforcement, metadata.
- [ ] **C4.12 Server-initiated calls and notifications** dispatched to registered handlers; missing handler → ERROR UNIMPLEMENTED.
- [ ] **C4.13 Reconnect policy** (new session on loss; resumption is M10) with backoff and jitter, surfaced as connection state events.
- [ ] **C4.14 Bundle size budget** declared and tracked in CI (pick a number and hold it).
- [ ] **C4.15 Background-tab liveness** (§7.4): PONG from the receive path, no timer-based server-death verdict without a probing PING; `bufferedAmount` high-water mark as send backpressure (§4.3).
- [ ] **C4.16 Pipelining in the client:** SUBSCRIBEs and CALLs issued right after HELLO are sent before WELCOME arrives and failed locally with `nx: true` on REJECT.
- [ ] **C4.17 Retry policy:** automatic retry only when `nx: true`, when the method is `idempotent`, or when an idempotency key was attached (§8.4, §8.7).

### Tests
- **Codec vectors:** every row of §12.3 in both codecs, both languages (Rust and TS must produce identical bytes for CBOR headers and equivalent values for data).
- **Client conformance runner:** the TS runtime against the fake server scripts (layer 3 client side).
- **Browser e2e (Playwright):** Chromium, Firefox, WebKit running an example app: typed unary calls, typed errors, server→client calls, notifications, cancellation, `bigint` round-trips, reconnect after server restart.
- **Fingerprint mismatch:** client built from an older manifest triggers the warning; `strict` rejects.
- **Background tab (Playwright):** hide the page for longer than `idle`, keep the server pinging; the session survives; then bring it back and verify no spurious reconnect happened. Repeat with the page in a `SharedWorker`-less mobile emulation.
- **Contract diff:** a fixture set of manifest pairs with expected breaking/compatible verdicts for every row of §12.7.

### Exit criteria
- Changing a Rust method signature and regenerating produces a TypeScript **compile error** at the call site in the example app. (This is the DX promise; test it literally.)
- Browser e2e green in all three engines.

**Rust focus:** traits with associated schema, derive macros (first real proc-macro work, scoped to type schema only), deterministic serialization, snapshot testing (`insta`).

---

## M5: Streaming shapes and WebSocket flow control

**Goal:** server streams, client streams and bidirectional calls in both directions, with real backpressure.

**Spec:** §6.3 (all shapes), §6.4, §8.3, §8.5-8.6 for streams.

### Capabilities
- [ ] **C5.1 Server stream** (shape 1): ITEM* then END; ERROR at any point; typed items.
- [ ] **C5.2 Client stream** (shape 2): caller ITEM* END; callee RESULT/ERROR; callee may terminate early and the caller stops sending.
- [ ] **C5.3 Bidirectional** (shape 3): both halves independent.
- [ ] **C5.4 All shapes server → client** (server-initiated streaming calls).
- [ ] **C5.5 WS credit** (§6.4): initial `window`; ITEM data consumes credit; CREDIT granted as the **application** consumes; sender blocks when out of credit; violation → PROTOCOL_ERROR; items larger than `window` rejected locally.
- [ ] **C5.6 Cancellation of streams:** caller CANCEL stops the producer promptly (measurable); callee early termination stops the caller's producer.
- [ ] **C5.7 Deadlines** cover the whole stream.
- [ ] **C5.8 Session end** cancels every stream in both directions, with local UNAVAILABLE on the caller side.
- [ ] **C5.9 Frames after terminal** → PROTOCOL_ERROR; frames for closed stream IDs ignored.
- [ ] **C5.10 TS runtime:** `AsyncIterable` in and out; `for await` + `break` sends CANCEL; backpressure honors consumer speed.
- [ ] **C5.11 Client-side `window` and `max_frame`** from HELLO govern server → client streams (§6.4); each side's limits bound what is sent to it.
- [ ] **C5.12 Sender scheduling** (§6.6): priority classes on the single WebSocket writer; coalescing flush on queue drain; WebSocket message size cap (§4.3).

### Tests
- **Scripts:** each shape × direction; empty streams; error mid-stream; cancel mid-stream (assert producer stopped via a counter); credit exhaustion and resumption; oversized item; frames after END.
- **Browser e2e:** each shape in all three engines; a slow `for await` consumer doesn't grow server memory.
- **Starvation test:** a saturating bulk stream in one direction while measuring PONG latency and unary-call latency on the same session; both stay bounded (proves §6.6).
- **Soak (first):** 1,000 sessions each running a long bidirectional stream with random cancels for 1 hour; memory flat; no leaked stream state.

### Exit criteria
- All shapes pass in both directions and both languages; soak clean.

**Rust focus:** `Stream`/`Sink`, pinning, bounded channels as backpressure, per-stream credit accounting without locks on the hot path.

---

## M6: Targeting: sessions, users, groups

**Goal:** the SignalR-class core: the server can reach any set of sessions, from anywhere in the application.

**Spec:** §13.1-13.6.

### Capabilities
- [ ] **C6.1 Targets** (§13.1): All, Session(s), User(s), Group(s), each with Except.
- [ ] **C6.2 Deduplication:** a session matched several ways receives a message once.
- [ ] **C6.3 Groups** (§13.2): add/remove idempotent, case-sensitive, per-session, cleared on session end.
- [ ] **C6.4 Users** (§13.3): user ID derived from the principal via a pluggable function; anonymous sessions never matched.
- [ ] **C6.5 Context handle:** a cloneable handle usable from any task (background jobs, plain HTTP handlers) supporting every target, group operations, and server-initiated calls to a single session.
- [ ] **C6.6 Broker boundary:** a trait/abstraction for "deliver to targets, group membership, session lookup" with an in-memory implementation. The multi-node implementation (M15) plugs in here; design it now.
- [ ] **C6.7 Lifecycle hooks** (§13.4): on-connect (before WELCOME, may reject, may join groups) and on-disconnect (exactly once, at session end).
- [ ] **C6.8 Outbound queue and slow consumers** (§13.5): bounded stream-0 queue, grace period, CLOSE SLOW_CONSUMER; PING/PONG/CLOSE bypass; one slow session never delays others.
- [ ] **C6.9 Encode once** (§13.6): a broadcast encodes its data section once per codec.
- [ ] **C6.10 Fan-out call helper** (optional): call a client method on many sessions and aggregate results (independent calls on the wire).
- [ ] **C6.11 Maximum session age** with jitter → GOAWAY (§7.6); configurable, on by default.

### Tests
- **Scripts with many sessions:** N raw clients; assert exact recipients per target, including "received nothing" assertions.
- **Browser e2e:** multi-tab chat (groups + direct messages + user targeting across two tabs of the same user).
- **Concurrency:** group churn during broadcasts; no delivery to a session after its end completes.
- **Slow consumer:** a client that stops reading is closed after the grace period; other clients' p99 latency stays bounded.
- **Benchmark (baseline):** broadcast to 10k sessions, recorded for later comparison.

### Exit criteria
- The chat example (rooms, DMs, per-user delivery) works in all browsers; slow-consumer policy proven.

**Rust focus:** concurrent maps and their contention profile, avoiding locks across `.await`, `Bytes` sharing for fan-out.

---

## M7: Axum integration and MVP hardening

**Goal:** something another Rust developer can drop into an axum app.

### Capabilities
- [ ] **C7.1 Mounting:** one call mounts an endpoint at a path; multiple endpoints per app.
- [ ] **C7.2 App state** reachable from handlers without depending on axum's `State`.
- [ ] **C7.3 Request context:** headers, cookies and extensions of the upgrade request available to the authenticator and on-connect hook.
- [ ] **C7.4 Coexistence** with user tower middleware (tracing, CORS for the well-known endpoint, compression that must not break upgrades).
- [ ] **C7.5 GOAWAY and drain** (§7.6): on shutdown, GOAWAY spread over a drain window with jittered `retry`, reject new calls with UNAVAILABLE `nx: true`, finish calls up to a deadline, then CLOSE GOING_AWAY. The TS client honors `retry` with its own jitter and migrates to a new session.
- [ ] **C7.6 Tracing:** spans per session and per call; method, status, duration; no payloads by default.
- [ ] **C7.7 Public API review:** no axum types in service-facing APIs; everything public documented.
- [ ] **C7.8 Examples:** chat, a live dashboard fed by a background task, a file upload via client streaming.
- [ ] **C7.9 Docs:** getting started (Rust + TS), codegen workflow, binding and codec notes, known limitations.

### Tests
- Full layer 1-4 suite against the examples mounted in a realistic router.
- Graceful shutdown with calls in flight: clients observe GOAWAY, finish or retry, and reconnect.
- **Thundering herd:** drain 5,000 sessions; reconnect arrivals at the replacement server are spread across the drain window, not clustered.

### Exit criteria (MVP)
- Conformance levels **Core**, **Streaming** and **Server calls** (spec §17.1) pass on WebSocket.
- Browser e2e green in three engines; soak clean; docs published in the repo.

---

## M8: WebTransport binding

**Goal:** the modern transport, with automatic fallback, and identical behavior on both bindings.

**Spec:** §4.2, §4.4, §4.5, §5.3, §6.3 (QUIC stream mapping), §6.5.

### Capabilities
- [ ] **C8.1 HTTP/3 server** on the same host and path, accepting WebTransport sessions (extended CONNECT). Choose the QUIC/WebTransport library; record why.
- [ ] **C8.2 Draft compatibility** `[verify]`: accept the WebTransport draft versions used by current Chromium, Firefox and Safari. Build a small compatibility table in the docs and keep it current.
- [ ] **C8.3 Control stream** = first client bidi stream; HELLO first; streams opened before WELCOME are reset.
- [ ] **C8.4 Call streams** = one QUIC bidi stream per call, both directions; FIN after terminal frames; RESET_STREAM ↔ CANCEL/UNAVAILABLE mapping (§6.3); STOP_SENDING on early callee termination.
- [ ] **C8.5 Limits:** QUIC `MAX_STREAMS` aligned with `max_calls`; QUIC flow control windows configured sensibly; no Pavia CREDIT frames on this binding.
- [ ] **C8.6 Session close** with Pavia close code and reason.
- [ ] **C8.7 Origin validation** on the CONNECT request.
- [ ] **C8.8 Development certificates:** a dev mode that generates a certificate meeting the browser constraints for `serverCertificateHashes` and prints the hash `[verify constraints]`.
- [ ] **C8.9 TS runtime WebTransport binding** with the same public API as WebSocket.
- [ ] **C8.10 Transport race** (§4.4): WebTransport first, WebSocket after the head-start or on failure; first WELCOME wins; loser closed; failure cache per origin.
- [ ] **C8.11 Binding independence:** application code is unaware of the binding; the active binding is observable for diagnostics.
- [ ] **C8.12 Deployment notes:** UDP exposure, load balancers that handle QUIC, container/ingress caveats, `Alt-Svc` not required for WebTransport.
- [ ] **C8.13 Pre-auth QUIC windows** (§4.2): small connection/stream flow-control limits until WELCOME, raised afterwards; pipelined streams are not read before WELCOME and are reset on REJECT.
- [ ] **C8.14 Stream priorities:** `sendOrder` on the client for control > lanes > calls `[verify]`; equivalent server-side priorities in the QUIC stack.
- [ ] **C8.15 QUIC idle alignment** (§4.2): `max_idle_timeout` > `idle`.
- [ ] **C8.16 `transports` hint** in WELCOME; client failure-cache shortening (§4.4).
- [ ] **C8.17 Connection migration:** verify what browsers actually do on a network change over WebTransport `[verify]`; document whether the session survives at the QUIC layer or falls to resumption (M10).

### Tests
- **Binding matrix:** the entire layer-3/4 suite runs on WebTransport; differences allowed only where spec §4.5 lists them.
- **Head-of-line test:** on WebTransport with injected packet loss, a stalled large stream does not delay an unrelated unary call (measure); on WebSocket it may (documented).
- **UDP blocked:** blackhole UDP → the client falls back to WebSocket within the head-start budget; the failure cache skips WebTransport next time; a later WELCOME over WebSocket listing `webtransport` shortens the cache.
- **Network switch:** change the client's source address mid-session (netns or `tc` tricks); record the observed behavior per browser and pin it in the docs.
- **Pre-auth DoS:** open 100 streams and push data before HELLO → bounded memory, streams reset on REJECT.
- **Browsers:** Chromium, Firefox, WebKit (Safari 26.4+) over WebTransport `[verify Playwright WebKit WebTransport support; otherwise test Safari manually or via a device farm]`.

### Exit criteria
- Conformance levels Core, Streaming, Server calls pass on **both** bindings; fallback proven under UDP blocking.

**Rust focus:** `quinn`-based stacks, mapping QUIC stream lifecycles onto your stream abstraction, running HTTP/1.1, HTTP/2 and HTTP/3 listeners together.

---

## M9: Datagrams and lanes

**Goal:** the features WebSocket-era protocols can't offer, with graceful emulation on WebSocket.

**Spec:** §9.2, §11.

### Capabilities: datagrams
- [ ] **C9.1 Topics from the contract** (§12.2): typed payloads, direction, mode `latest`/`all`.
- [ ] **C9.2 DGRAM_BIND** key spaces per direction; binding limit; unbound-key datagrams dropped silently.
- [ ] **C9.3 WebTransport datagrams** (§11.3) with per-key seq; `latest` receivers drop stale seq.
- [ ] **C9.4 Size limits:** min(`max_dgram`, transport `maxDatagramSize`); oversized rejected locally, never fragmented.
- [ ] **C9.5 WebSocket emulation** (§11.4): coalescing for `latest` (one pending per key), low priority, drop `all`-mode under pressure.
- [ ] **C9.6 Server handlers** for inbound topics; server-side targeting (§13.1) for outbound; inbound rate limiting per session.
- [ ] **C9.7 TS API:** typed `send` per topic; typed subscription to inbound topics.

### Capabilities: lanes
- [ ] **C9.8 LANE_OPEN** and unidirectional lane streams on WebTransport; lane key on WebSocket (§9.2).
- [ ] **C9.9 Lane selection API** on the server's send path and in the TS client (e.g. lane from a key hash).
- [ ] **C9.10 Lane limits** per direction; excess → PROTOCOL_ERROR.

### Tests
- **Loss and reorder** (UDP faults): `latest` never regresses; `all` loses but never corrupts.
- **Coalescing on WebSocket:** a burst of 1,000 cursor updates on a slow link delivers the final value promptly with bounded queue size.
- **Lane independence on WebTransport:** a stalled lane doesn't delay another lane; ordering within a lane preserved.
- **Abuse:** datagram flood → rate limit applied; bind-table exhaustion → PROTOCOL_ERROR.
- **Demo:** collaborative cursors example (datagrams over channels in M12 later; for now server-relayed to a group).

### Exit criteria (Headline 0.x)
- Datagram and lane behavior proven on both bindings; the cursors demo runs smoothly in all three browsers.
- **Announce** (r/rust, This Week in Rust, Hacker News, relevant TypeScript communities) with benchmarks from M6 re-run on both bindings.

---

## M10: Session resumption

**Goal:** short transport interruptions (tunnels, Wi-Fi hiccups, WebSocket↔WebTransport switches) lose nothing.

**Spec:** §7.8, §13.4 (hooks don't run on resume).

### Capabilities
- [ ] **C10.1 Capability `resume`**; resume token issuance and rotation on every WELCOME.
- [ ] **C10.2 Sequenced frames** (§7.8): every control-stream and lane frame except the exempt list carries `seq`; counters are **per lane, per direction**; client control requests (SUBSCRIBE, AUTH_REFRESH, DGRAM_BIND...) are sequenced too.
- [ ] **C10.3 Acknowledgment** via `ack` maps, as ACK frames and piggybacked on sequenced frames; cadence 1 s / 64 frames; replay-buffer trimming per lane, both sides.
- [ ] **C10.4 Replay buffer bounds** with backpressure or loss of resumability when full (documented choice).
- [ ] **C10.5 Detached state:** transport loss without CLOSE detaches the session; targeted sends buffer; groups and subscriptions persist; `resume_window` expiry ends the session (on-disconnect runs, groups cleared).
- [ ] **C10.6 Resume handshake:** HELLO `resume` → WELCOME `resumed: true` + `seq`; retransmission from the peer's position; duplicate discard.
- [ ] **C10.7 Principal binding:** different principal or expired token → REJECT RESUME_FAILED; different codec or version → RESUME_FAILED; authorization re-evaluated after resume (§7.5).
- [ ] **C10.7a Detached limits:** per-principal and total caps on detached sessions; resume-attempt rate limit per address.
- [ ] **C10.7b Server calls to detached sessions** fail immediately with UNAVAILABLE `nx: true`.
- [ ] **C10.8 Calls at transport loss** complete locally with UNAVAILABLE; handlers cancelled; TS client retries calls marked `idempotent`.
- [ ] **C10.9 Cross-binding resume:** start on WebTransport, resume on WebSocket, and the reverse.
- [ ] **C10.10 TS runtime:** transparent resumption with state events (`reconnecting`, `resumed`, `new-session`).

### Tests
- **Fault injection:** kill the transport during a notification burst at random points, 1,000 iterations; zero loss, zero duplicates, order preserved per lane.
- **Expiry:** resume after `resume_window` → RESUME_FAILED → new session → on-disconnect ran exactly once for the old one.
- **Security:** replaying an old resume token fails; resuming from a different user fails; opening 9 tabs and dropping them all keeps at most 8 detached sessions.
- **Lanes under resumption (WebTransport):** drop the transport while three lanes carry traffic at different rates; each lane replays exactly its gap; LANE_OPEN is replayed first.
- **Lost control request:** drop the transport right after the client sends SUBSCRIBE; after resume the SUBSCRIBE is replayed and SUBSCRIBED arrives exactly once.

### Exit criteria
- Conformance level **Resumption** passes on both bindings; fault-injection loop clean.

**Rust focus:** a sequenced, acknowledged buffer with bounded memory; session state that outlives its transport task; careful ownership handoff between old and new transport tasks.

---

## M11: Authentication lifecycle, authorization and interceptors

**Spec:** §7.5, §8.2 step 4, §10.2 step 4, §14.

### Capabilities
- [ ] **C11.1 AUTH_EXPIRING** notices driven by credential expiry reported by the authenticator.
- [ ] **C11.2 AUTH_REFRESH / AUTH_RESULT** in-band; different principal → FORBIDDEN; session continues with old credential until expiry.
- [ ] **C11.3 Expiry enforcement:** no refresh → CLOSE UNAUTHENTICATED (with configurable grace).
- [ ] **C11.4 Authorization hooks** per method (with access to principal, method, metadata, and decoded arguments) and per channel subscription.
- [ ] **C11.5 Interceptor pipeline** around calls, notifications and session lifecycle: global and per-service, deterministic order, can short-circuit with a status, can observe/transform errors, applies to streaming calls at call start (document whether items are intercepted).
- [ ] **C11.6 TS runtime:** token provider callback; automatic refresh on AUTH_EXPIRING.
- [ ] **C11.7 Authorization re-evaluation** on AUTH_REFRESH, on resume, and on an optional interval or policy signal (§7.5); revoked channels get UNSUBSCRIBED code 1.
- [ ] **C11.8 Per-session rate limits** completed: control requests, notifications, datagrams (§16), all closing with LIMIT_EXCEEDED and each covered by the abuse suite.

### Tests
- Expiring JWTs in browser e2e (short lifetimes); refresh keeps a long-running stream alive; refresh with another user's token is refused; per-method denial codes; interceptor ordering table tests.

### Exit criteria
- A long-lived session survives several token rotations without reconnecting.

---

## M12: Channels, history and recovery

**Spec:** §10.1-10.5.

### Capabilities
- [ ] **C12.1 Channel namespaces** from the contract with typed publications and subscription params.
- [ ] **C12.2 SUBSCRIBE processing order** (§10.2): validation, duplicates, limits, authorization.
- [ ] **C12.3 UNSUBSCRIBE** (client) and server-initiated UNSUBSCRIBED with reason codes (§10.3).
- [ ] **C12.4 Publishing API** from handlers and the context handle; publications encoded once per codec.
- [ ] **C12.5 History store abstraction** with an in-memory implementation: bounded by count and age; epochs; offsets +1 per publication.
- [ ] **C12.6 Recovery** (§10.5): `since` → `recovered: true` + replay, or `recovered: false`.
- [ ] **C12.7 Replay/live handoff without gaps or duplicates** (§10.2 last paragraph).
- [ ] **C12.8 Continuity loss** → UNSUBSCRIBED code 2 to subscribers; new epoch.
- [ ] **C12.9 Resumption interplay:** channel frames are sequenced (M10); a resumed session receives exactly what it missed.
- [ ] **C12.10 TS runtime:** typed subscriptions, automatic resubscribe with `since` after new sessions, a "stale state" callback when `recovered: false`.
- [ ] **C12.11 Snapshot-on-subscribe** (§10.2): per-namespace `snapshot` type; snapshot taken exactly at SUBSCRIBED `offset` (no publication between snapshot and offset); mandatory when `recovered: false` and the namespace declares a snapshot.
- [ ] **C12.12 Lossy-with-recovery** (§13.5): over-threshold sessions drop pending history-channel publications and receive UNSUBSCRIBED code 2 before any SLOW_CONSUMER decision.
- [ ] **C12.13 Single sequencer per channel** (§10.5): the history-store abstraction assigns offsets atomically; document the invariant so M15 doesn't break it.

### Tests
- Recovery matrix: within history, beyond history, wrong epoch, after server restart with a persistent store stub.
- Race test: publish at high rate while subscribers join with `since`; assert gapless, duplicate-free sequences.
- Snapshot consistency: publish continuously while subscribing with snapshot; applying snapshot + publications > offset yields the same state as a subscriber that was there from the start.
- Slow subscriber on a history channel is not disconnected; it gets code 2, resubscribes and converges.
- Browser e2e: chat rebuilt on channels; reload the page and recover missed messages.

### Exit criteria
- Conformance levels **Channels** and **History** pass on both bindings.

---

## M13: Presence

**Spec:** §10.6.

### Capabilities
- [ ] **C13.1 Membership tracking** per channel with server-supplied presence info.
- [ ] **C13.2 Snapshot on subscribe** (when requested), before replayed publications.
- [ ] **C13.3 Join/leave deltas**, batched where possible.
- [ ] **C13.4 Leave on session end** (including `resume_window` expiry), not on transport loss that resumes.
- [ ] **C13.5 Presence limits** (large channels): configurable cap on snapshot size or presence disabled per namespace.
- [ ] **C13.6 TS runtime:** reactive member list per channel, grouped by user if requested.
- [ ] **C13.7 Presence `update`** (kind 3) when a member's info changes without leaving.

### Tests
- Join/leave storms with 1,000 sessions; final membership exact; resumed sessions don't flap; expired sessions leave exactly once.

### Exit criteria
- Conformance level **Presence** passes; the chat example shows accurate online users across reloads and network drops.

---

## M14: Clients: Rust client and TypeScript extras

### Capabilities
- [ ] **C14.1 Rust client** reusing the sans-IO protocol crate: WebSocket and WebTransport, typed via the same Rust definitions (Rust ↔ Rust with no codegen step).
- [ ] **C14.2 Client conformance:** the Rust client passes the client-runner scripts.
- [ ] **C14.3 TS multi-tab sharing** via SharedWorker where available (one session per origin shared by tabs), falling back to per-tab sessions `[verify SharedWorker availability per browser, notably mobile]`.
- [ ] **C14.4 Offline queue** for notifications (bounded, opt-in) flushed on reconnect.
- [ ] **C14.5 Retry policy** for idempotent calls with backoff; never for non-idempotent calls.
- [ ] **C14.6 Framework adapters** (optional): thin React/Svelte/Vue bindings for connection state, subscriptions and presence.

### Tests
- Rust client ↔ server full scenario suite; multi-tab e2e (open 5 tabs → 1 session; close the owning tab → another tab takes over).

---

## M15: Multi-node

**Goal:** many server nodes behave as one endpoint.

### Capabilities
- [ ] **C15.1 Distributed broker** implementing the M6 boundary over a message system (choose NATS or Redis; record why). Every target type works across nodes, delivered once.
- [ ] **C15.2 Remote group operations:** adding a session that lives on another node to a group is forwarded to its node and acknowledged.
- [ ] **C15.3 Session directory:** which node owns which session, for targeting and server-initiated calls across nodes.
- [ ] **C15.4 Resumption across nodes:** a resuming client may land on a different node. Choose and document: (a) route resumes to the owning node (load-balancer affinity by resume token or QUIC connection ID), or (b) migrate detached session state through the shared store. This is the hardest multi-node decision; write it into the spec.
- [ ] **C15.5 Shared history store** (e.g. Redis streams, NATS JetStream) implementing the M12 abstraction; epochs survive node restarts.
- [ ] **C15.6 Distributed presence** with eventual consistency and guaranteed leave on node death (heartbeated node leases).
- [ ] **C15.7 Failure behavior:** broker outage policy (fail sends vs buffer) documented and tested.
- [ ] **C15.8 Deployment guide:** load balancing for WebSocket and QUIC (QUIC connection-ID-aware balancing for migration), affinity requirements, sizing.
- [ ] **C15.9 Sequencer ownership:** how a channel's single sequencer is chosen and moved across nodes (store-assigned offsets vs channel ownership with lease), and how an epoch change is triggered on failover.
- [ ] **C15.10 Distributed idempotency store** for `pavia-idempotency-key` (§8.7).

### Tests
- 3 nodes + broker in docker compose: every target type across nodes; kill a node mid-broadcast; restart the broker; resume onto another node; presence converges after node death.
- Multi-node soak: 10k sessions, 1 hour, churn.

### Exit criteria (Feature complete)
- Every scenario in the suite passes on a 3-node cluster, on both bindings.

---

## M16: Production hardening

### Capabilities
- [ ] **C16.1 Metrics:** sessions (by binding, codec), calls (by method, status, latency), notifications, publications, datagrams (sent, dropped, coalesced), queue depths, resumptions, slow-consumer closes.
- [ ] **C16.2 OpenTelemetry:** trace context from `meta.traceparent` into server spans; client-side spans in the TS runtime (optional).
- [ ] **C16.3 Limits review:** every limit in spec §16 configurable, enforced and covered by a test.
- [ ] **C16.4 Security review** against spec §14: token handling, Origin checks, DoS surfaces (handshake flooding, varint and CBOR bombs, stream and lane exhaustion, datagram floods, presence enumeration).
- [ ] **C16.5 Continuous fuzzing** in CI for all decoders; the abuse suite runs against every limit with the limit set to its default.
- [ ] **C16.6 Benchmarks** with published methodology: memory per idle session, broadcast latency at 10k/50k sessions, unary call throughput, stream throughput, on both bindings, compared with ASP.NET Core SignalR, socketioxide and Centrifugo on the same hardware.

---

## M17: Ergonomics

### Capabilities
- [ ] **C17.1 `#[service]` / `#[client]` macros** turning Rust traits into registrations and manifest entries; hand-written registration remains available.
- [ ] **C17.2 Typed server-side client proxies:** calling a client method or sending a notification is a typed Rust call, not a string.
- [ ] **C17.3 Typed channel and datagram handles** on the server side.
- [ ] **C17.4 Error ergonomics:** `?`-friendly mapping from user error types to status codes and typed details.
- [ ] **C17.5 Documentation site:** guides for each feature, a migration guide for SignalR users (concept mapping table), deployment guide, spec rendered alongside.

---

## M18: 1.0

### Capabilities
- [ ] **C18.1 Spec freeze:** resolve or explicitly defer every §18 open question; tag protocol version 1 as stable.
- [ ] **C18.2 Public conformance suite:** vectors and scripts packaged so third parties can test their own clients and servers (e.g. Swift, Kotlin, .NET clients).
- [ ] **C18.3 Semver and MSRV policy** for crates; versioning policy for the TS packages; compatibility guarantees between client and server versions.
- [ ] **C18.4 Compatibility matrix** published (below), all green.

### Exit criteria (1.0)
- Every conformance level passes on both bindings, both codecs, with the TS and Rust clients, on single-node and multi-node deployments.

---

## Appendix A: Conformance matrix (track in the README)

| Level | WS / cbor | WS / json | WT / cbor | WT / json | Multi-node |
|---|---|---|---|---|---|
| Core | | | | | |
| Streaming | | | | | |
| Server calls | | | | | |
| Datagrams | | | | | |
| Resumption | | | | | |
| Channels | | | | | |
| History | | | | | |
| Presence | | | | | |

Each cell is run with the TS client (three browser engines) and the Rust client.

## Appendix B: Recurring pitfalls

- **Frames are the unit, not WebSocket messages or QUIC reads.** Always decode incrementally.
- **Deterministic CBOR in headers** or your vectors will flake.
- **No cross-stream ordering.** Tests that pass on WebSocket because TCP happens to order things will fail on WebTransport. Run the binding matrix early and often.
- **Credit is granted on consumption, not arrival**, or backpressure silently disappears.
- **PING/PONG must bypass every queue and limit**, or healthy sessions time out under load.
- **`i64` in JavaScript** is `bigint`; any code path that turns it into `number` is a bug.
- **Replay/live handoff** in channel recovery is where gaps and duplicates hide; test it under load.
- **Resumption across nodes** decides your load-balancer story; don't postpone thinking about it past M10.
- **Every `[verify]` item** about browsers or WebTransport drafts gets a test before you rely on it.
- **`nx` discipline:** the moment a handler starts, `nx` can never be `true` for that call. Any code path that forgets this turns safe retries into duplicate side effects.
- **Per-lane sequencing** is not optional once lanes exist; a single counter looks fine on WebSocket and silently breaks on WebTransport.
- **Pre-auth is where DoS lives:** every buffer that exists before WELCOME needs a small hard cap.
- **Scope:** every milestone after M9 is optional for a useful library. If motivation dips, ship what you have.
