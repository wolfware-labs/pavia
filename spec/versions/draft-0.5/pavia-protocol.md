# Pavia Wire Protocol Specification

**Version:** 1 (draft 0.5; see the Changelog at the end)
**Status:** Working draft. Nothing here is frozen until the 1.0 milestone.

---

## 0. Conventions

- The key words MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are used as in RFC 2119 / RFC 8174.
- "Peer" means either endpoint. "Client" initiates the session; "server" accepts it.
- Byte values are hexadecimal (`0x10`). Sizes use KiB/MiB (1024-based).
- CBOR examples use CBOR diagnostic notation (RFC 8949 §8).
- **[open]** marks a design decision that is deliberately unresolved and listed in §18.
- **[verify]** marks a statement about third-party behavior (browsers, libraries, drafts) that must be confirmed during implementation.

---

## 1. Overview

Pavia is a real-time RPC and messaging protocol between browsers/apps and servers. It keeps SignalR's connection model (a persistent session; either side can call the other; server-side targeting of sessions, users and groups) and adds:

- **Typed contracts** in both directions, described by a machine-readable manifest (§12) so clients can be generated.
- **Multiplexed calls** with four shapes (unary, server stream, client stream, bidirectional), cancellation, deadlines, metadata and structured errors.
- **Modern transports:** WebTransport over HTTP/3 as the preferred binding, WebSocket as the universal fallback, with one logical protocol over both.
- **Datagrams** for lossy, latest-value data.
- **Channels** with durable history, gapless recovery and presence.
- **Session resumption** across transport loss.

### 1.1 Goals
1. One logical protocol, two bindings (WebTransport, WebSocket), with identical application semantics except where §4 states otherwise.
2. Binary, compact and cheap to route: envelopes are parseable without decoding user payloads, so fan-out serializes each payload once.
3. Evolvable: unknown extension frames and unknown header keys can be ignored; versions and capabilities are negotiated.
4. Implementable by third parties from this document plus the test vectors (§17).

### 1.2 Non-goals (v1)
- Compatibility with SignalR, Socket.IO, gRPC or MQTT wire formats.
- SSE or long-polling transports.
- Client-to-client messaging without a server hop.
- Payload compression (a flag bit is reserved, §5.2).
- Media delivery (see Media over QUIC for that problem space).

---

## 2. Terminology

| Term | Meaning |
|---|---|
| **Session** | A logical connection between one client and one server, identified by a session ID. Survives transport replacement via resumption (§7.8). |
| **Transport** | The underlying WebTransport session or WebSocket connection carrying a session. |
| **Stream** | An independent, ordered, bidirectional sequence of frames. On WebTransport, a QUIC stream; on WebSocket, a logical stream multiplexed by stream ID. |
| **Control stream** | Stream 0. Carries session, notification, channel and (on WebSocket) datagram frames. |
| **Call stream** | A stream carrying exactly one call (§8). |
| **Frame** | The protocol's unit of transmission (§5). |
| **Header section** | A CBOR map inside a frame carrying protocol fields. |
| **Data section** | Opaque bytes inside a frame carrying the user payload, encoded with the session codec. |
| **Codec** | The encoding of data sections: `cbor` or `json` (§5.5). |
| **Service** | A named set of methods, e.g. `chat`. |
| **Server method** | A method implemented by the server and invoked by clients. |
| **Client method** | A method implemented by the client and invoked by the server. |
| **Notification** | A fire-and-forget method invocation (§9). |
| **Group** | A server-side, non-durable set of sessions used for targeting. Invisible to the protocol. |
| **Channel** | A client-visible subscription topic with optional history and presence (§10). |
| **Lane** | An ordering domain for notifications (§9.2). |
| **Contract** | The set of services, client methods, channels and datagram topics an endpoint exposes, described by a manifest (§12). |

### 2.1 Layering

```
+-------------------------------------------------------------+
| Contract (manifest, types, codegen)                    §12   |
+------------------+------------------+-----------+-----------+
| Calls        §8  | Notifications §9 | Channels  | Datagrams |
|                  | + lanes          | §10       | §11       |
+------------------+------------------+-----------+-----------+
| Session: handshake, heartbeat, auth, resumption, close  §7  |
+-------------------------------------------------------------+
| Streams: IDs, lifecycle, flow control, limits           §6  |
+-------------------------------------------------------------+
| Framing: varints, frames, header/data sections          §5  |
+------------------------------+------------------------------+
| WebTransport binding    §4.2 | WebSocket binding       §4.3 |
+------------------------------+------------------------------+
```

---

## 3. Endpoints

An Pavia **endpoint** is a single URL path (e.g. `https://example.com/rt`). One session on an endpoint can reach every service registered there. Unlike SignalR hubs, services do not require separate connections.

- The same path MUST be usable for both bindings: WebTransport via HTTP/3 extended CONNECT, WebSocket via upgrade.
- An endpoint MAY publish its contract manifest at `GET {path}/.well-known/pavia/contract` (§12.6).

---

## 4. Transport bindings

### 4.1 Common requirements
- Both bindings MUST run over TLS (`https`/`wss`), except `ws://` on loopback for development.
- The server MUST validate the `Origin` header on both bindings against a configured allow-list when browser clients are expected (§14).
- Authentication credentials MUST NOT be sent in the URL. They are sent in HELLO (§7.1). Cookies attached by the browser to the upgrade/CONNECT request MAY also be used.
- Exactly one session runs on one transport. A transport carries no frames before HELLO and none after CLOSE.
- **TLS early data (0-RTT):** servers MUST NOT process HELLO or any other frame received in TLS 1.3 / QUIC 0-RTT early data, because early data can be replayed. Servers either disable 0-RTT or defer reading until the handshake completes.
- **Pre-WELCOME pipelining:** after sending HELLO, a client MAY send further frames without waiting for WELCOME (this saves one round trip on every connect and resume). The server MUST NOT process them before authentication succeeds; it buffers them subject to a **pre-authentication buffer limit** (default 64 KiB) and closes with LIMIT_EXCEEDED if it is exceeded. On REJECT, everything pipelined is discarded and the client library fails pending calls locally with UNAVAILABLE (§8.4, `nx: true`). Servers MUST NOT send anything but WELCOME, REJECT or CLOSE before WELCOME.

### 4.2 WebTransport binding
- The client establishes a WebTransport session to `https://host{path}` (HTTP/3 extended CONNECT, `:protocol = webtransport`).
- **Draft divergence [verify]:** as of 2026, browsers do not all speak the same WebTransport draft (Chromium and Firefox have used an early draft; Safari and the current IETF draft a later one). A conforming server MUST accept every draft version used by the browsers it claims to support. This is a transport-library concern; the Pavia protocol is identical across drafts.
- **Control stream:** the client MUST open one client-initiated bidirectional stream immediately after the session is established. This is stream 0. Its first frame MUST be HELLO. The client MAY open further streams before receiving WELCOME (pipelining, §4.1); the server MUST NOT read or act on them until it has sent WELCOME, and MUST reset them on REJECT. To bound pre-authentication memory, the server SHOULD keep QUIC connection and stream flow-control windows small (e.g. 64 KiB total) until WELCOME and raise them afterwards. A server receiving a stream before the control stream exists MUST wait for the control stream (up to the handshake timeout) rather than treating it as an error.
- **Subprotocol:** clients SHOULD request `pavia.1` via the WebTransport `protocols` option where the browser supports it, and servers select it; a mismatch is a REJECT UNSUPPORTED_VERSION after the control stream opens [verify browser support].
- **Idle alignment:** the QUIC `max_idle_timeout` MUST be greater than Pavia `idle` (§7.4), otherwise QUIC ends the session before Pavia can decide whether it is a transport loss. Recommended: `idle` + 15 s.
- **Send order:** where the runtime exposes stream priorities (the WebTransport `sendOrder` option), the control stream MUST have the highest priority, followed by lane streams, then call streams (§6.6) [verify support].
- **Call streams:** each call uses a new bidirectional QUIC stream opened by the caller (client or server). Stream IDs are QUIC's; no Pavia stream ID appears on the wire.
- **Lane streams:** each non-zero lane uses a unidirectional stream opened by the sender (§9.2).
- **Datagrams:** use WebTransport datagrams (§11.3).
- **Flow control and stream limits** are QUIC's. The server SHOULD configure its QUIC `MAX_STREAMS` limits to match the `max_calls` it advertises in WELCOME.
- **Framing on a stream:** frames are concatenated without stream ID prefix (§5.3).
- **Closing:** a session is closed with a WebTransport session close carrying the Pavia close code (§15.1) as the application error code and a UTF-8 reason.
- **Local development:** browsers accept self-signed certificates for WebTransport only via the `serverCertificateHashes` option, which imposes certificate constraints (short validity, specific key types) [verify current constraints]. Server tooling SHOULD provide a development mode that generates a compliant certificate and prints its hash.

### 4.3 WebSocket binding
- The client connects to `wss://host{path}` with `Sec-WebSocket-Protocol: pavia.1`. The server MUST select `pavia.1` or refuse the upgrade.
- HTTP/1.1 upgrade MUST be supported by servers. WebSockets over HTTP/2 (RFC 8441) and HTTP/3 (RFC 9220) MAY be supported.
- The subprotocol name identifies the **framing** version (§5); protocol semantics within that framing are negotiated by `versions` in HELLO (§7.1). A new framing would be `pavia.2`.
- An HTTP/1.1 request on the endpoint path that is not a WebSocket upgrade MUST be answered `426 Upgrade Required` (with `Upgrade: websocket`). Over HTTP/2 and HTTP/3, a request on the path that is not an extended CONNECT MUST be answered `405 Method Not Allowed` (with `Allow: CONNECT`). The well-known sub-path (§12.6) is exempt from both rules.
- Servers MUST NOT negotiate `permessage-deflate` in v1 (compression of credential-bearing frames alongside attacker-influenced data enables CRIME/BREACH-class attacks; see §18 for future compression).
- Only **binary** WebSocket messages are allowed. A text message is a PROTOCOL_ERROR.
- A WebSocket message contains **one or more complete frames**, each prefixed by its stream ID (§5.4). A frame MUST NOT be split across WebSocket messages. Senders MAY batch frames of different streams into one message. A WebSocket message MUST NOT exceed `2 × max_frame` bytes; receivers enforce this before buffering (PROTOCOL_ERROR).
- **Client send backpressure:** the browser `WebSocket` API has no send backpressure. A client runtime MUST watch `bufferedAmount` (or use `WebSocketStream` where available) and treat a high-water mark (default 1 MiB) as "no credit": it stops producing ITEMs and applies the datagram coalescing rules (§11.4) until the buffer drains.
- All streams, including stream 0, are multiplexed by stream ID (§6.1).
- Per-stream flow control uses CREDIT frames (§6.4).
- Datagrams are emulated with DATAGRAM frames on stream 0 (§11.4).
- Lane streams are multiplexed like call streams and provide no independence on WebSocket, because TCP delivers everything in order (§9.2).
- **Closing:** send CLOSE (§7.7), then a WebSocket close with code `4000 + pavia_close_code` (§15.1).
- **Inbound backpressure:** a server whose unprocessed inbound bytes for a session exceed a limit (default 1 MiB) stops reading the socket, letting TCP apply backpressure. While it is not reading it MUST pause its own idle timer for that session (it cannot see PONGs), and it MUST resume reading within the slow-consumer grace period (§13.5) or close with SLOW_CONSUMER.

### 4.4 Transport selection (client behavior)

Clients that support both bindings SHOULD use this algorithm:

1. If WebTransport is available in the runtime and the origin is not in the client's **WebTransport failure cache**, start a WebTransport attempt.
2. Start a WebSocket attempt either when the WebTransport attempt fails or after a **head-start delay** (default 250 ms), whichever comes first.
3. The first attempt to receive WELCOME wins. The loser is closed with close code NORMAL (or abandoned if not yet established).
4. If WebTransport failed or lost the race, record the origin in the failure cache for a configurable period (default 24 h), so future sessions go straight to WebSocket.
5. Resumption (§7.8) MAY switch bindings: a session started on WebTransport can resume on WebSocket and vice versa.
6. WELCOME MAY carry `transports` (key 46), the bindings the server offers on this endpoint. A client that connected over WebSocket and sees `webtransport` listed SHOULD clear the failure cache for the origin sooner (default: retry WebTransport on the next connect after 1 h); a lost race is not proof that WebTransport is blocked.

Rationale: some networks block UDP, and a failed QUIC handshake can take seconds to time out. Racing hides that delay.

### 4.5 Binding differences (normative summary)

| Aspect | WebTransport | WebSocket |
|---|---|---|
| Head-of-line blocking between calls | None | Present (TCP) |
| Flow control | QUIC native | CREDIT frames |
| Datagrams | Real, unreliable | Emulated, reliable, coalesced |
| Lanes | Independent streams | Accepted, no independence |
| Network migration (Wi-Fi ↔ cellular) | QUIC connection migration [verify browser support] | Requires resumption |
| Stream IDs on wire | No | Yes |

---

## 5. Framing

### 5.1 Variable-length integers
All integers in frame prefixes use the QUIC variable-length integer encoding (RFC 9000 §16): the two most significant bits of the first byte give the length (1, 2, 4 or 8 bytes); values range up to 2^62−1. Encoders MUST use the shortest encoding. Decoders MUST accept any valid encoding in length fields but MUST reject values exceeding configured limits before allocating.

### 5.2 Frame layout

```
Frame {
  Type          (8 bits),
  Flags         (8 bits),
  Length        (varint),       // length of Payload in bytes
  Payload {
    HeaderLength (varint),      // length of Header in bytes; may be 0
    Header       (HeaderLength bytes, CBOR map),
    Data         (Length - size(HeaderLength) - HeaderLength bytes)
  }
}
```

- **Type:** see the registry (§5.6).
- **Flags:** all bits reserved in v1. Senders MUST set them to 0. Receivers MUST ignore unknown flag bits. Bit 0 is reserved for a future "data compressed" flag (§18, item 1, deferred).
- **Header:** a CBOR map (§5.4.1). An empty header is encoded as HeaderLength = 0 (not as an empty map).
- **Data:** opaque bytes interpreted per frame type and the session codec. May be empty.
- **`max_frame`** (§7.1, §7.2) bounds the Length field, that is the payload bytes: the HeaderLength varint, the header and the data. The Type, Flags and Length prefix and, on WebSocket, the stream ID (§5.4) are not counted. A receiver checks Length against `max_frame` before buffering the payload.

### 5.3 Frames on WebTransport streams
Frames are written back-to-back on the stream. A receiver reads Type, Flags, Length, then Length bytes. A stream ending (FIN) in the middle of a frame is a PROTOCOL_ERROR.

### 5.4 Frames on WebSocket

```
WsFrame {
  StreamId (varint),
  Frame    (as §5.2)
}
```

A WebSocket binary message is a sequence of one or more `WsFrame`s. Leftover bytes that don't form a complete WsFrame are a PROTOCOL_ERROR.

#### 5.4.1 Header section encoding
- The header is a CBOR map with **unsigned integer keys** from the key registry (§5.7), except where a field is defined as a nested map with text keys.
- Senders MUST use CBOR core deterministic encoding (RFC 8949 §4.2.1): definite lengths, shortest integer forms, sorted keys. This makes golden test vectors byte-exact.
- Receivers MUST reject indefinite-length items in headers, MUST enforce a maximum header size (default 16 KiB) and nesting depth (default 16), and MUST ignore unknown keys.
- Receivers SHOULD accept non-deterministic but otherwise valid encodings.

### 5.5 Data section and codecs
The codec is negotiated in the handshake (§7.1-7.2) and applies to every data section in the session.

| Codec | Requirement | Data section |
|---|---|---|
| `cbor` | MUST be supported by all implementations; default | One CBOR data item |
| `json` | SHOULD be supported; intended for debugging | One JSON value, UTF-8 text |

The mapping of contract types to each codec is defined in §12.3. The header section is **always CBOR** regardless of codec, so routers, backplanes and logs can parse envelopes without decoding payloads.

### 5.6 Frame type registry

Types `0x00-0x7F` are **must-understand**: an unknown type in this range is a PROTOCOL_ERROR. Types `0x80-0xFF` are **extensions**: unknown ones MUST be ignored (the frame is skipped using Length). Type 0x80 is reserved for a future METHOD_BIND extension frame (§18, item 2).

| Type | Name | Stream | Direction | Section |
|---|---|---|---|---|
| 0x01 | HELLO | 0 | C→S | §7.1 |
| 0x02 | WELCOME | 0 | S→C | §7.2 |
| 0x03 | REJECT | 0 | S→C | §7.3 |
| 0x04 | PING | 0 | both | §7.4 |
| 0x05 | PONG | 0 | both | §7.4 |
| 0x06 | GOAWAY | 0 | S→C | §7.6 |
| 0x07 | CLOSE | 0 | both | §7.7 |
| 0x08 | AUTH_REFRESH | 0 | C→S | §7.5 |
| 0x09 | AUTH_RESULT | 0 | S→C | §7.5 |
| 0x0A | ACK | 0 | both | §7.8 (also piggybacked via key 44) |
| 0x0B | AUTH_EXPIRING | 0 | S→C | §7.5 |
| 0x0C | REQUEST_ERROR | 0 | S→C | §7.9 |
| 0x10 | CALL | call | both | §8.2 |
| 0x11 | ITEM | call | both | §8.3 |
| 0x12 | END | call | both | §8.3 |
| 0x13 | RESULT | call | callee→caller | §8.4 |
| 0x14 | ERROR | call | callee→caller | §8.4 |
| 0x15 | CANCEL | call | caller→callee | §8.5 |
| 0x16 | CREDIT | call (WS only) | both | §6.4 |
| 0x20 | NOTIFY | 0 or lane | both | §9 |
| 0x21 | LANE_OPEN | lane | both | §9.2 |
| 0x30 | SUBSCRIBE | 0 | C→S | §10.2 |
| 0x31 | SUBSCRIBED | 0 | S→C | §10.2 |
| 0x32 | UNSUBSCRIBE | 0 | C→S | §10.3 |
| 0x33 | UNSUBSCRIBED | 0 | S→C | §10.3 |
| 0x34 | PUBLICATION | 0 | S→C | §10.4 |
| 0x35 | PRESENCE | 0 | S→C | §10.6 |
| 0x40 | DATAGRAM | 0 (WS) / datagram (WT) | both | §11 |
| 0x41 | DGRAM_BIND | 0 | both | §11.2 |

A frame received on the wrong kind of stream, or in the wrong direction, is a PROTOCOL_ERROR.

### 5.7 Header key registry

| Key | Name | CBOR type | Used in |
|---|---|---|---|
| 1 | `rid` | uint | Control requests/responses (§7.9) |
| 2 | `method` | text | CALL, NOTIFY |
| 3 | `meta` | map text → (text / bytes) | CALL, RESULT, ERROR, END, NOTIFY, SUBSCRIBE |
| 4 | `deadline` | uint (ms, relative) | CALL |
| 5 | `shape` | uint (§8.1) | CALL |
| 6 | `code` | uint | REJECT, ERROR, CLOSE, GOAWAY, REQUEST_ERROR, UNSUBSCRIBED, AUTH_RESULT |
| 7 | `msg` | text | same as `code` |
| 8 | `lane` | uint | LANE_OPEN |
| 9 | `channel` | text | channel frames |
| 10 | `epoch` | text | SUBSCRIBED, SUBSCRIBE `since` |
| 11 | `offset` | uint | SUBSCRIBED, PUBLICATION, SUBSCRIBE `since` |
| 12 | `seq` | uint | Sequenced frames (§7.8; per lane, per direction), DATAGRAM (per key) |
| 13 | `token` | bytes or text | auth, resume |
| 14 | `versions` | array of uint | HELLO |
| 15 | `version` | uint | WELCOME |
| 16 | `codecs` | array of text | HELLO |
| 17 | `codec` | text | WELCOME |
| 18 | `caps` | array of text | HELLO, WELCOME |
| 19 | `session` | text | WELCOME, presence entries |
| 20 | `resume` | map {13: token, 44: ack, 53: lost?} | HELLO |
| 21 | `limits` | map uint → uint: {48: max_calls, 49: max_frame, 50: max_subs, 51: max_dgram, 40: window} (§7.2) | HELLO, WELCOME |
| 22 | `hb` | uint (ms) | WELCOME |
| 23 | `idle` | uint (ms) | WELCOME |
| 24 | `contract` | bytes (32) | HELLO, WELCOME |
| 25 | `credit` | uint (bytes) | CREDIT |
| 26 | `recovered` | bool | SUBSCRIBED |
| 27 | `presence` | bool | SUBSCRIBE |
| 28 | `kind` | uint | PRESENCE |
| 29 | `key` | uint | DGRAM_BIND, DATAGRAM (WS) |
| 30 | `name` | text | DGRAM_BIND, `client` map |
| 31 | `retry` | uint (ms) | REJECT, GOAWAY |
| 32 | `url` | text | GOAWAY |
| 33 | `auth` | map {35: scheme, 13: token} | HELLO, AUTH_REFRESH |
| 34 | `client` | map {30: name, 52: version} | HELLO (e.g. `{30: "pavia-ts", 52: "0.3.0"}`) |
| 35 | `scheme` | text | inside `auth` (§7.1), e.g. `"bearer"` |
| 36 | `user` | text | WELCOME (optional echo of authenticated user ID) |
| 37 | `resume_window` | uint (ms) | WELCOME |
| 38 | `expires` | uint (ms until expiry) | AUTH_RESULT, AUTH_EXPIRING |
| 39 | (unassigned) | | Was `last_stream`; removed in draft 0.3 |
| 40 | `window` | uint (bytes) | inside `limits` (§6.4) |
| 41 | `since` | map {10: epoch, 11: offset} | SUBSCRIBE |
| 43 | `resumed` | bool | WELCOME |
| 44 | `ack` | map uint → uint (lane → highest contiguous seq received) | ACK; MAY be piggybacked on any sequenced frame (§7.8) |
| 45 | `nx` | bool | ERROR: `true` means the callee guarantees the handler never ran (§8.4) |
| 46 | `transports` | array of text (`"websocket"`, `"webtransport"`) | WELCOME (§4.4) |
| 48 | `max_calls` | uint | inside `limits` |
| 49 | `max_frame` | uint (bytes) | inside `limits` |
| 50 | `max_subs` | uint | inside `limits` |
| 51 | `max_dgram` | uint (bytes) | inside `limits` |
| 52 | `version` (text) | text | inside `client` |
| 53 | `lost` | array of uint (lane numbers) | WELCOME, `resume` map (§7.8) |

Keys 1-1023 are reserved for this specification. Keys ≥ 1024 are available for extensions and MUST be ignored if unknown. Keys 39 and 42 are unassigned; unassigned keys below 1024 MUST NOT be used by extensions.

---

## 6. Streams

### 6.1 Stream identifiers (WebSocket)
- Stream **0** is the control stream. It exists from the start of the transport and is never closed independently of the session.
- Client-initiated call and lane streams use **even** IDs ≥ 2; server-initiated ones use **odd** IDs ≥ 1.
- Each initiator's IDs MUST be strictly increasing. IDs MUST NOT be reused within a transport.
- A stream is opened by the first frame carrying a new ID. That frame MUST be CALL (call stream) or LANE_OPEN (lane stream).
- Frames for a stream ID that was opened and has since closed MUST be ignored (to tolerate races). Frames for a stream ID that was never opened, other than an opening frame, are a PROTOCOL_ERROR.
- On resumption over a new transport, stream ID numbering restarts; streams do not survive transport loss (§7.8).

### 6.2 Control stream rules
- The first frame from the client MUST be HELLO; the first frame from the server MUST be WELCOME or REJECT.
- The server sends no frame other than WELCOME, REJECT or CLOSE before the handshake completes. The client MAY pipeline frames after HELLO (§4.1); they are processed only after WELCOME, in order.
- On WebTransport, a FIN or reset of the control stream that was not preceded by CLOSE is a transport loss (§7.7): the session detaches if resumption is enabled and ends otherwise. Only CLOSE ends a session with no possibility of resumption.

### 6.3 Call stream lifecycle

Each call stream has two halves: **caller → callee** (request half) and **callee → caller** (response half).

| Shape | Request half | Response half |
|---|---|---|
| Unary (0) | CALL (half closed by CALL itself) | RESULT or ERROR |
| Server stream (1) | CALL (half closed by CALL itself) | ITEM* then END, or ERROR at any point |
| Client stream (2) | CALL, ITEM*, END | RESULT or ERROR |
| Bidirectional (3) | CALL, ITEM*, END | ITEM* then END, or ERROR at any point |

- **Terminal frames:** RESULT, ERROR and callee-side END close the response half. END and CANCEL close the request half.
- A call is **complete** when both halves are closed. On WebTransport, each side also FINs its QUIC send side after its terminal frame.
- The callee MAY close the response half (RESULT/ERROR) before the request half is closed, e.g. rejecting a client stream early. The caller MUST then stop sending ITEMs; on WebTransport the callee SHOULD send STOP_SENDING.
- Any frame after a terminal frame in the same half is a PROTOCOL_ERROR.
- On WebTransport, a QUIC stream reset (RESET_STREAM) by the caller MUST be treated as CANCEL; by the callee, as ERROR with code UNAVAILABLE unless an Pavia terminal frame was already received.

### 6.4 Flow control

**WebTransport:** QUIC stream and connection flow control apply. Pavia adds nothing.

**WebSocket:** per-stream credit applies to **call streams only**.
- Each call stream starts with a send window of `window` bytes in each direction: frames sent **to the server** use the server's `window` (WELCOME `limits.window`), frames sent **to the client** use the client's `window` (HELLO `limits.window`, default 256 KiB). The same rule applies to `max_frame`: each side's advertised value bounds frames sent to it.
- Only **ITEM data sections** consume credit. CALL, RESULT, ERROR, END, CANCEL and CREDIT do not.
- A sender MUST NOT send an ITEM whose data section exceeds its remaining window. It waits for CREDIT.
- A receiver grants more window with `CREDIT {25: increment}` on the same stream ID. Receivers SHOULD grant credit as the application consumes items, not as bytes arrive, so that backpressure reaches the sender.
- An ITEM larger than the sender's current window waits for CREDIT like any other. A receiver MUST eventually grant a blocked stream at least `max_frame` bytes of credit as the application consumes earlier items, so any item up to `max_frame` can be sent. An item whose data section exceeds `max_frame` is rejected locally with RESOURCE_EXHAUSTED and never sent.
- The ITEM bytes in flight toward a peer are bounded by `max_calls × window` for that direction; there is no session-level credit (§18, item 7).
- Exceeding the window is a PROTOCOL_ERROR.
- Stream 0 has no credit. Its backpressure is governed by the slow-consumer policy (§13.5).

### 6.5 Concurrency limits
- WELCOME `limits.max_calls` is the maximum number of concurrently open call streams the **client** may initiate. HELLO `limits.max_calls` is the same for server-initiated calls.
- WebTransport: enforced by QUIC `MAX_STREAMS`; the advertised value is informational.
- WebSocket: a CALL exceeding the limit MUST be answered immediately with ERROR RESOURCE_EXHAUSTED (`nx: true`); the stream then closes normally.
- **Open/cancel rate:** independent of concurrency, servers MUST rate-limit stream opens and CANCELs per session (default 1,000 per 10 s combined). A CALL immediately followed by CANCEL is cheap for the attacker and not for the server (the HTTP/2 "rapid reset" pattern). Exceeding the rate → CLOSE LIMIT_EXCEEDED.

### 6.6 Sender scheduling
A transport has one writer; without rules, a bulk stream starves the frames that keep the session healthy.
- Priority classes, highest first: (1) PING, PONG, CLOSE; (2) CANCEL, CREDIT, RESULT, ERROR, END, ACK; (3) control-stream frames (NOTIFY, channel frames, DGRAM_BIND, AUTH_*); (4) ITEM and CALL data, round-robin across streams; (5) emulated datagrams (§11.4).
- On WebSocket the sender MUST apply this ordering when choosing the next frame to write. On WebTransport the same order is expressed through stream priorities (§4.2); PING/PONG/CLOSE are on the control stream, which has the highest priority.
- **Coalescing:** senders SHOULD flush when the queue drains rather than per frame, so that bursts of small frames share one WebSocket message or QUIC packet without adding latency.

---

## 7. Session

### 7.1 HELLO (0x01, C→S)

First frame on the control stream. Header fields:

| Key | Field | Req. | Notes |
|---|---|---|---|
| 14 | `versions` | MUST | Protocol versions the client supports, e.g. `[1]` |
| 16 | `codecs` | MUST | In preference order, e.g. `["cbor", "json"]` |
| 18 | `caps` | SHOULD | Capabilities requested (§7.10) |
| 21 | `limits` | SHOULD | Client-side limits applying to frames sent to the client, same keys as in WELCOME (§7.2): `max_calls` (server-initiated calls), `max_frame`, `window` (WS credit), `max_dgram` |
| 24 | `contract` | SHOULD | SHA-256 fingerprint of the manifest the client was generated from (§12.5) |
| 33 | `auth` | MAY | `{35: "bearer", 13: "<token>"}` |
| 20 | `resume` | MAY | `{13: resume_token, 44: ack_map, 53: lost?}` to resume a session (§7.8) |
| 34 | `client` | SHOULD | Client library name and version |

Data section: empty in v1.

The server MUST receive a complete HELLO within the **handshake timeout** (default 10 s), or close with HANDSHAKE_TIMEOUT.

### 7.2 WELCOME (0x02, S→C)

| Key | Field | Req. | Notes |
|---|---|---|---|
| 15 | `version` | MUST | Highest version present in both sides' lists |
| 17 | `codec` | MUST | First codec in the client's list the server supports |
| 18 | `caps` | MUST | Capabilities granted (subset of requested) |
| 19 | `session` | MUST | Session ID: public, stable across resumption, unique per endpoint across all nodes (≥ 96 bits of randomness or a node-prefixed counter), text, URL-safe `[A-Za-z0-9_-]`, ≤ 64 chars. Never a credential. |
| 13 | `token` | MUST if `resume` cap granted | Resume token: secret, ≥128 bits random, rotated on every WELCOME |
| 21 | `limits` | MUST | Server limits, a map with integer keys: `max_calls` (48), `max_frame` (49), `max_subs` (50), `max_dgram` (51), and on WebSocket `window` (40, initial per-stream credit, §6.4). Prose in this document writes them as `limits.max_frame` and so on |
| 22 | `hb` | MUST | Heartbeat interval in ms |
| 23 | `idle` | MUST | Idle timeout in ms |
| 37 | `resume_window` | MUST if `resume` granted | How long the server keeps a detached session |
| 24 | `contract` | SHOULD | Server's manifest fingerprint |
| 36 | `user` | MAY | Authenticated user ID, if any |
| 43 | `resumed` | MUST when resuming | `true` if HELLO.resume succeeded |
| 44 | `ack` | MUST when resumed | Per-lane highest contiguous seq the server received from the client (§7.8) |
| 53 | `lost` | MUST when resumed and frames toward the client were evicted | Lanes with a gap (§7.8) |
| 46 | `transports` | MAY | Bindings offered on this endpoint (§4.4) |

- The server MUST NOT send WELCOME before authentication (if required) has succeeded.
- After WELCOME, both sides may open streams and send any frame permitted by the granted capabilities.
- **Contract mismatch:** if fingerprints differ, the server applies its policy: `warn` (default; proceed) or `strict` (REJECT with CONTRACT_MISMATCH). Clients SHOULD surface a mismatch warning to developers.

### 7.3 REJECT (0x03, S→C)
Sent instead of WELCOME. Fields: `code` (§15.1), `msg`, optional `retry` (ms before retrying). The server closes the transport after sending REJECT. Typical codes: UNSUPPORTED_VERSION, UNAUTHENTICATED, FORBIDDEN, RESUME_FAILED, CONTRACT_MISMATCH, LIMIT_EXCEEDED (e.g. server at capacity, with `retry`).

A client receiving REJECT with RESUME_FAILED MUST start a new session (HELLO without `resume`); all session-scoped state is lost, and channels must be recovered via §10.5.

### 7.4 Heartbeat: PING (0x04) and PONG (0x05)
- Each peer MUST send PING if it has sent nothing on the transport for `hb` ms (default 15,000).
- PING's data section MAY carry up to 8 opaque bytes; PONG MUST echo them. This allows RTT measurement.
- A peer MUST answer every PING with PONG promptly, even under backpressure.
- If a peer receives nothing at all for `idle` ms (default 45,000), it MUST close with IDLE_TIMEOUT (or, if resumption is enabled, treat the transport as lost, §7.8).
- Any received frame, on any stream, resets the idle timer.
- **Server-driven liveness for browsers:** browsers throttle timers in background tabs (to as little as once per minute), so a client's own PING timer is unreliable. A client MUST answer PING from its receive path (never from a timer), and a browser client SHOULD NOT declare the server dead purely on a timer: after a suspected gap it sends PING and waits one `hb` for PONG before treating the transport as lost. Servers therefore drive liveness: they PING on schedule and judge the client by PONGs. Server implementations SHOULD tolerate late PONGs from clients that report a browser user agent by using the full `idle` window rather than a shorter PONG deadline.
- **PING rate limit:** a peer receiving more than 10 PINGs in `hb` ms without any other traffic SHOULD close with LIMIT_EXCEEDED (the gRPC "too many pings" lesson).

### 7.5 Authentication
- **Initial:** credentials in HELLO `auth`, or cookies on the upgrade request. The server validates them before WELCOME. Failure → REJECT UNAUTHENTICATED or FORBIDDEN.
- **Principal:** the authenticated principal is fixed for the session. On resumption, the resumed transport's credentials (if any) MUST resolve to the same principal, or the server MUST REJECT with RESUME_FAILED.
- **Expiry notice:** the server MAY send `AUTH_EXPIRING {38: ms_until_expiry}` before the credential expires.
- **Refresh:** `AUTH_REFRESH {1: rid, 33: auth}` replaces the credential in-band. The server answers `AUTH_RESULT {1: rid, 6: code, 38: expires?}` where code 0 = accepted. A malformed AUTH_REFRESH (missing or unknown fields, unknown scheme) is answered with REQUEST_ERROR INVALID_ARGUMENT (§7.9). A credential the authenticator evaluated and refused is answered with AUTH_RESULT carrying UNAUTHENTICATED (invalid or expired) or PERMISSION_DENIED (a **different** principal); in both cases the session continues with the old credential until it expires.
- **Expiry without refresh:** the server MUST close the session with UNAUTHENTICATED (servers MAY allow a short grace period).
- **Re-evaluation:** authorization decisions are not permanent. After a successful AUTH_REFRESH and after every resumption, the server MUST re-evaluate the authorization of every active channel subscription (UNSUBSCRIBED code 1 on failure). Servers SHOULD also re-check on a configurable interval or when their policy source signals a change.

### 7.6 GOAWAY (0x06, S→C)
Announces server draining (deploys, rebalancing).
- Fields: `code` (usually GOING_AWAY), `msg`, optional `url` (endpoint to reconnect to), optional `retry`.
- After GOAWAY the server MUST answer every new client-initiated CALL, on either binding, with ERROR UNAVAILABLE (`nx: true`) and MUST let calls in progress finish until a drain deadline. The client learns per call which requests were not processed; no stream-ID watermark is needed.
- The client SHOULD open a new session (to `url` if given), move new traffic to it, and close the old session once its calls complete.
- When draining ends, the server sends CLOSE GOING_AWAY.
- GOAWAY does not use resumption: the new session is a new session. Servers that want state continuity SHOULD rely on channel recovery (§10.5) and the backplane.
- **Staggered draining:** when draining many sessions (deploy, scale-in), a server MUST spread GOAWAYs over a drain window and SHOULD set `retry` with per-session jitter, so reconnects do not arrive as a thundering herd. Clients MUST honor `retry` and add their own jitter (±20 %).
- **Maximum session age:** servers SHOULD send GOAWAY after a configurable maximum session age (default 4 h ± 10 % jitter) so that long-lived sessions rebalance across nodes after scale-outs (the gRPC `MAX_CONNECTION_AGE` lesson). Clients treat this like any other GOAWAY.

### 7.7 CLOSE (0x07, both)
- Fields: `code` (§15.1), optional `msg`.
- The sender closes the transport after CLOSE (WebSocket close code `4000 + code`; WebTransport session close with `code`).
- A session closed with CLOSE is **not** resumable, whatever the code.
- A transport that drops without CLOSE is a **transport loss**: the session becomes detached and resumable if resumption is enabled (§7.8); otherwise it ends.

### 7.8 Session resumption (capability `resume`)

Resumption preserves session identity and server-side session state across a transport loss.

**Preserved:** session ID, authenticated principal, group memberships, channel subscriptions, datagram key bindings, and all **sequenced** frames not yet acknowledged.
**Not preserved:** call streams. Every call in progress at transport loss is terminated: the caller's library completes it locally with UNAVAILABLE, and the callee's handler is cancelled. Clients MAY retry calls marked idempotent in the contract.

**Sequenced frames:** every frame on the control stream and on lane streams **except** HELLO, WELCOME, REJECT, PING, PONG, ACK, GOAWAY, CLOSE and DATAGRAM. That is: NOTIFY, LANE_OPEN, AUTH_REFRESH, AUTH_RESULT, AUTH_EXPIRING, REQUEST_ERROR, SUBSCRIBE, SUBSCRIBED, UNSUBSCRIBE, UNSUBSCRIBED, PUBLICATION, PRESENCE and DGRAM_BIND, in both directions. (Client control requests must be sequenced too; otherwise a SUBSCRIBE lost in a transport drop leaves the client waiting forever, and DGRAM_BIND state could be "preserved" without ever having arrived.)

When `resume` is granted, each sequenced frame MUST carry `seq` (key 12). Counters are **per lane, per direction**: lane 0 (the control stream) and each lane stream have their own counter starting at 1 and incrementing by 1 per sequenced frame. Per-lane counters are required because on WebTransport lanes are delivered independently, so a single counter across lanes could never be acknowledged contiguously.

**Acknowledgment:**
- Each peer acknowledges with an `ack` map (key 44): lane → highest seq n such that all frames 1..n on that lane have been received. It is sent either as an `ACK` frame on stream 0 or piggybacked as key 44 on any sequenced frame the peer is sending anyway (receivers MUST accept both forms). Lanes omitted from the map are unchanged.
- A peer SHOULD acknowledge at least every 1 s while it has unacknowledged receipts, or after every 64 sequenced frames, whichever comes first.
- Senders keep sequenced frames in a **replay buffer** until acknowledged. The buffer is bounded (default 1 MiB per session per direction).
- **Buffer pressure:** a sequenced send never blocks and the bound is never exceeded. When a frame would not fit, the sender evicts buffered frames in this order until it does: (1) PUBLICATION frames for channels with history, oldest first; the sender drops that subscription and queues one `UNSUBSCRIBED {code: 2}` (§10.3) for the channel in their place, so the client re-syncs from history with `since`. (2) PRESENCE frames, because a resubscribe yields a fresh snapshot. (3) Any other sequenced frame, oldest first; each such eviction records a **gap** on the frame's lane. Recoverable traffic goes first; every other eviction is reported as a gap.
- **Gaps:** the resuming WELCOME carries `lost` (key 53), the lanes on which frames toward the client were evicted; the client's `resume` map carries `lost` for the other direction. For each listed lane the receiver sets its receive position to one less than the seq of the first frame it then receives on that lane, and accepts that frame although it is not contiguous with its previous position. Client libraries surface a gap event per lane; applications decide what a missed notification means for them.

**Resuming:**
1. The client detects transport loss (error, close without CLOSE, or idle timeout).
2. It opens a new transport (any binding, §4.4) and sends HELLO with `resume {13: token, 44: ack_map}`, where `ack_map` is the client's per-lane receive position. The HELLO MUST offer the same protocol version and codec as the original session (the replay buffer is already encoded); the server MUST REJECT RESUME_FAILED otherwise.
3. If the session exists, is within `resume_window` (default 30 s), and the principal matches, the server replies WELCOME with the **same** session ID, a **new** resume token, `resumed: true`, and `ack` = its per-lane receive position for frames from the client.
4. Both peers retransmit, per lane and in original order with original seq values, every buffered frame with seq greater than the peer's acknowledged value for that lane. Lane streams are reopened on the new transport (LANE_OPEN is itself sequenced and is replayed first on its lane). Receivers discard any frame whose seq they have already processed; on a lane listed in `lost` they re-anchor as described above.
5. The server re-evaluates authorization (§7.5). Normal operation continues.

If the session is unknown, expired, the principal differs, or the codec/version differs: REJECT RESUME_FAILED.

**Server behavior while detached:** sends targeted at the session are appended to the replay buffer, evicting under pressure as described above. The session still counts toward its groups and channel subscriptions. Server-initiated **calls** to a detached session fail immediately with UNAVAILABLE and `nx: true` (there is no transport to carry them; callers may retry after the session resumes). On `resume_window` expiry the session ends: groups are left, subscriptions dropped, presence leave events emitted.

**Limits:** servers MUST cap detached sessions per principal (default 8) and in total; when exceeded, the oldest detached session ends early. Resume attempts are rate-limited per source address like handshakes (§14).

**Resume token security:** single-use (rotated on every WELCOME), bound to the principal, never logged, and compared in constant time.

### 7.9 Control requests and REQUEST_ERROR
Control-stream operations that expect a reply (AUTH_REFRESH, SUBSCRIBE, UNSUBSCRIBE) carry a request ID `rid`, unique among the sender's outstanding requests. The reply carries the same `rid`. A failed request is answered with `REQUEST_ERROR {1: rid, 6: status_code, 7: msg}` using call status codes (§15.2).

### 7.10 Capabilities
Capabilities are negotiated in HELLO/WELCOME `caps`. The server grants a subset of what the client requested.

| Capability | Meaning |
|---|---|
| `calls` | Calls and notifications (always granted in v1) |
| `server-calls` | The server may call client methods |
| `streams` | Stream shapes 1-3 |
| `resume` | Session resumption (§7.8) |
| `channels` | Channels (§10) |
| `history` | Channel history and recovery (§10.5) |
| `presence` | Presence (§10.6) |
| `datagrams` | Datagrams (§11) |
| `lanes` | Lanes (§9.2) |
| `idempotency` | Server honors `pavia-idempotency-key` (§8.7) |

Using a feature whose capability was not granted is a PROTOCOL_ERROR.

---

## 8. Calls

### 8.1 Methods and shapes
- A method is addressed as `"<service>/<method>"`, e.g. `"chat/send"`. Names match `[A-Za-z_][A-Za-z0-9_]*` per segment and are **case-sensitive**.
- Server methods and client methods are separate namespaces: a CALL from the client is resolved among server methods, and a CALL from the server among client methods.
- Shapes: `0` unary, `1` server stream, `2` client stream, `3` bidirectional. "Server" and "client" here mean callee and caller: shape 1 means the **callee** streams to the **caller**, whichever side is the server.

### 8.2 CALL (0x10)
Opens a call stream. Header:

| Key | Field | Req. | Notes |
|---|---|---|---|
| 2 | `method` | MUST | |
| 5 | `shape` | MUST | Must match the contract's shape for this method |
| 4 | `deadline` | MAY | Milliseconds from the moment the callee receives CALL |
| 3 | `meta` | MAY | Call metadata (§8.7) |

Data section: the call's **arguments**, encoded as one codec array whose elements are the parameters in contract order. A method without parameters uses an empty array.

Callee checks, in order (each failure answers with ERROR and closes the call):
1. Concurrency limit exceeded (WS, §6.5) → RESOURCE_EXHAUSTED. This check runs before the frame's header and data are inspected any further.
2. Unknown method → UNIMPLEMENTED.
3. Shape mismatch → INVALID_ARGUMENT.
4. Arguments fail to decode or have the wrong count → INVALID_ARGUMENT.
5. Authorization fails → PERMISSION_DENIED (or UNAUTHENTICATED for an anonymous session).

### 8.3 ITEM (0x11) and END (0x12)
- ITEM: header empty (or `meta` on the first item, reserved); data section = one stream element, encoded per the contract's item type for that direction.
- END: closes the sender's half **successfully**. Header MAY carry `meta` as trailers. Data section empty.
- For client streams and bidirectional calls, the caller sends END to signal "no more items".
- For server streams and bidirectional calls, the callee sends END to signal successful completion.

### 8.4 RESULT (0x13) and ERROR (0x14)
- RESULT (callee → caller, shapes 0 and 2): data section = the return value. A method returning unit encodes the codec's unit value (§12.3). Header MAY carry `meta` as trailers.
- ERROR (callee → caller, any shape, at any point): header `code` (§15.2, never 0) and `msg`; MAY carry `meta`. Data section MAY carry a **typed error detail**, encoded per the method's declared error type (§12.2). If the method declares none, the data section MUST be empty.
- **`nx` (key 45):** an ERROR with `nx: true` guarantees that the method handler was never invoked, so the caller may retry even a non-idempotent call transparently. Callees MUST set it for UNIMPLEMENTED, INVALID_ARGUMENT, PERMISSION_DENIED, UNAUTHENTICATED and RESOURCE_EXHAUSTED raised before dispatch, and for UNAVAILABLE raised after GOAWAY or against a detached session. Client libraries set it on locally generated errors that provably preceded any send. An ERROR without `nx` means the outcome is unknown; only calls marked `idempotent` (or carrying an idempotency key, §8.7) may be retried automatically.
- **Error disclosure:** servers MUST NOT put internal error text in `msg` for unexpected failures unless configured for development; they use code INTERNAL with a generic message. Errors the application raises deliberately (typed errors or explicit status codes) are sent as given.

### 8.5 CANCEL (0x15)
- Caller → callee, at any time before the response half closes. Header MAY carry `msg`.
- The callee MUST stop work promptly and MUST reply with ERROR CANCELLED unless it has already sent a terminal frame.
- After sending CANCEL, the caller MUST ignore everything on the stream except the terminal frame, which completes cleanup.
- A callee that receives CANCEL after it has already sent its terminal frame MUST ignore it (the two crossed in flight). A second CANCEL on the same stream is ignored likewise.
- On WebTransport, the caller MAY also reset the stream; the callee treats a reset as CANCEL (§6.3).

### 8.6 Deadlines
- The callee starts a timer when it receives CALL with `deadline`. At expiry it cancels the handler and replies ERROR DEADLINE_EXCEEDED.
- The caller's library SHOULD also enforce the deadline locally (send CANCEL, complete with DEADLINE_EXCEEDED) to cover lost responses.
- Servers MAY enforce a maximum deadline and a default deadline for calls without one.
- For streaming shapes, the deadline covers the whole call, not individual items.

### 8.7 Metadata
- `meta` maps names to text or bytes. Names are lowercase ASCII `[a-z0-9-]`. Total encoded size counts toward the header limit.
- Reserved names:
  - `traceparent`, `tracestate`: W3C Trace Context. Implementations SHOULD propagate them into their tracing systems.
  - `pavia-idempotency-key`: an opaque caller-chosen key (≤ 128 bytes). A server that supports it MUST return the stored outcome of an earlier call with the same key from the same principal instead of executing again, for a retention window it documents (default 5 min), and MUST answer with the stored RESULT/ERROR. This lets clients retry non-idempotent calls after transport loss. Servers advertise support with capability `idempotency`.
  - `pavia-*`: reserved for this specification.
- Metadata is available to handlers and filters. It is never interpreted by the protocol otherwise.

### 8.8 Server-initiated calls (capability `server-calls`)
- The server opens a call stream to the client with CALL naming a **client method**. The same shapes and rules apply with roles reversed.
- The client library dispatches to the handler registered for that method. No handler → ERROR UNIMPLEMENTED.
- The client advertises in HELLO `limits.max_calls` how many concurrent server-initiated calls it accepts.

### 8.9 Ordering guarantees
- Frames within one stream are delivered in order.
- There is **no ordering between streams**, including between a call's RESULT and notifications on the control stream. (This differs from SignalR, where everything on a connection is ordered.) An application that needs "notify after result" ordering must enforce it itself, e.g. by awaiting the result before notifying.
- On WebSocket, cross-stream order happens to be preserved in practice; applications MUST NOT rely on it, because the same code must work on WebTransport.

---

## 9. Notifications

### 9.1 NOTIFY (0x20)
A fire-and-forget method invocation, in either direction.
- Header: `method` (MUST), `meta` (MAY), `seq` (MUST when `resume` is granted). The lane is implied by the stream the frame travels on (§9.2).
- Data section: arguments as a codec array, as in CALL.
- The receiver never replies. Errors (unknown method, bad arguments, handler failure) are logged by the receiver and otherwise ignored.
- **Delivery:** at-most-once without resumption; with resumption, in-order and exactly-once within a lane for the lifetime of the session.
- Server → client notifications are the result of server-side targeting (§13). Client → server notifications invoke server methods declared with notification semantics in the contract.

### 9.2 Lanes (capability `lanes`)
A lane is an ordering domain for notifications. Notifications on the same lane are delivered in send order; different lanes are independent.
- Lane 0 is the control stream and is always available.
- **WebTransport:** a sender opens a unidirectional stream for lane N (N ≥ 1) the first time it uses it; the first frame is `LANE_OPEN {8: N}`, followed by NOTIFY frames without a `lane` key. Lane streams stay open for the session's lifetime. A lost/reset lane stream is a transport error for the session.
- **WebSocket:** a sender opens a lane stream for lane N with a new stream ID (§6.1) whose first frame is `LANE_OPEN {8: N}`, followed by NOTIFY frames, exactly as on WebTransport. Lane streams are accepted for compatibility but give no independence, because TCP delivers everything in order.
- Lane numbers are chosen by the sending application (typically by hashing a key such as a document ID). Implementations MUST cap the number of lanes per direction (default 16) and treat excess as PROTOCOL_ERROR.
- Sequence numbers (§7.8) are per lane. LANE_OPEN is sequenced on its own lane with seq 1.

---

## 10. Channels (capability `channels`)

### 10.1 Model
- A channel is named by a UTF-8 string of at most 255 bytes, characters `[A-Za-z0-9:_\-./@]`. The part before the first `:` is its **namespace**, which maps to a channel declaration in the contract (§12.2), fixing the publication type and presence info type.
- Clients subscribe; the server authorizes each subscription.
- Publications are produced by the server (typically from a server method handler or a background job). Clients do not publish to channels directly in v1 **[open]**.
- Channels are distinct from groups: groups are server-side targeting sets with no client visibility, history or presence.

### 10.2 SUBSCRIBE (0x30) and SUBSCRIBED (0x31)
SUBSCRIBE header: `rid` (MUST), `channel` (MUST), `since` (MAY, requires `history`), `presence` (MAY, requires `presence`), `meta` (MAY). Data section: optional subscription parameters, typed per the channel declaration (empty if none).

Server processing:
1. Validate name and namespace → REQUEST_ERROR INVALID_ARGUMENT.
2. Already subscribed on this session → REQUEST_ERROR ALREADY_EXISTS.
3. Subscription limit (`max_subs`) → REQUEST_ERROR RESOURCE_EXHAUSTED.
4. Authorization → REQUEST_ERROR PERMISSION_DENIED.
5. Register the subscription, then reply SUBSCRIBED.

SUBSCRIBED header: `rid`, `channel`, `seq` (if resuming enabled), `epoch` (text), `offset` (the channel's current top offset) and `recovered` (only if `since` was given).

SUBSCRIBED data section: if the channel namespace declares a `snapshot` type (§12.2), the current state of the channel **exactly as of `offset`**, encoded with the session codec; otherwise empty. Snapshot-on-subscribe removes the "fetch state, then subscribe" race: the client applies the snapshot and then every publication with offset > SUBSCRIBED `offset`. When `since` was given and recovery succeeds, the server MAY omit the snapshot (the replay suffices); when recovery fails (`recovered: false`) a server that supports snapshots MUST include one, which makes the failure self-healing.

Frames that follow, in this order:
1. If presence was requested: a PRESENCE snapshot (§10.6).
2. If `since` was given and recovery succeeded: replayed PUBLICATION frames with offsets from `since.offset + 1` up to SUBSCRIBED `offset`, in order.
3. Live PUBLICATION frames with offsets greater than SUBSCRIBED `offset`.

The server MUST ensure no publication is lost or duplicated between the replay and live phases (e.g. by buffering live publications for this subscriber until the replay has been written).

### 10.3 UNSUBSCRIBE (0x32) and UNSUBSCRIBED (0x33)
- Client: `UNSUBSCRIBE {1: rid, 9: channel}`. Server replies `UNSUBSCRIBED {1: rid, 9: channel}`. Unsubscribing a channel that isn't subscribed is not an error.
- Server-initiated: `UNSUBSCRIBED {9: channel, 6: code, 7: msg}` without `rid`. Codes (unsubscribe reasons, separate registry, §15.3): `0` normal, `1` permission revoked, `2` resubscribe required, `3` channel closed.
- On code 2 the client SHOULD resubscribe immediately, using `since` if it has a position.
- After UNSUBSCRIBED the server MUST NOT send further PUBLICATION or PRESENCE frames for that channel on this session.

### 10.4 PUBLICATION (0x34)
- Header: `channel`, `offset`, and `seq` when resumption is enabled.
- Data section: the publication value, typed by the channel declaration.
- Offsets are unsigned integers, strictly increasing by exactly 1 per publication within an epoch.

### 10.5 History and recovery (capability `history`)
- Every channel has an epoch and an offset counter whether or not it retains history; `history` controls only how far back `since` can reach. A channel without history answers every `since` with `recovered: false`.
- A channel with history keeps its most recent publications, bounded by count and/or age (server configuration).
- An **epoch** identifies a continuous history. It changes whenever history continuity is broken (history store lost, channel reset). Clients compare epochs as opaque strings.
- **Recovery:** a client that saw publications up to `(epoch E, offset N)` subscribes with `since {10: E, 11: N}`.
  - If the server still has epoch E and publications N+1 onward: `recovered: true` and those publications are replayed.
  - Otherwise: `recovered: false`, no replay. The client MUST treat its local state for that channel as stale and re-fetch it by other means (typically a server method returning a snapshot plus the offset it corresponds to).
- Clients SHOULD track `(epoch, offset)` per subscribed channel from every SUBSCRIBED and PUBLICATION they process.
- Recovery works across session loss, node failure and server restarts, provided the history store survives. This is its purpose; session resumption (§7.8) covers only short transport interruptions.
- If a server loses continuity for a live channel, it MUST send UNSUBSCRIBED code 2 (resubscribe) to its subscribers.
- **Sequencing constraint:** because offsets increase by exactly 1, every channel with history needs a single sequencer (one owner assigning offsets) at any moment, even in multi-node deployments. Implementations choose the mechanism (channel ownership, a store that assigns offsets atomically such as an append-only log); the protocol only requires the result. An offset gap observed by a client is a server bug, not a recoverable condition.

### 10.6 Presence (capability `presence`)
- PRESENCE header: `channel`, `kind` (`0` snapshot, `1` join, `2` leave, `3` update), `seq` when resumption is enabled. `update` carries entries whose `info` changed (e.g. a display name) without leaving.
- Data section: a codec array of entries, each `{session: text, user: text | null, info: <presence info type> | null}`.
- Snapshot: all current members. Join/leave: the entries that joined or left.
- A session is a member while subscribed with or without `presence: true`; `presence: true` only controls whether it **receives** presence frames. **[open]** whether to allow "invisible" subscriptions.
- Presence info is supplied by the server when the subscription is authorized (e.g. display name). Clients cannot set it directly in v1.
- Multiple sessions of the same user appear as separate entries; clients group by `user` if they want per-user presence.
- On multi-node deployments, presence is eventually consistent. Implementations MUST ensure that a session that leaves (or whose node dies) eventually produces a leave event.

---

## 11. Datagrams (capability `datagrams`)

### 11.1 Model
Datagrams carry small, frequent, disposable values: cursors, typing indicators, positions, telemetry. They are declared in the contract as **datagram topics** (§12.2) with a payload type, a direction and a mode:
- `latest`: only the newest value per key matters; older ones may be dropped.
- `all`: every value matters, but loss is tolerated (best effort).

### 11.2 Keys and DGRAM_BIND (0x41)
To keep datagrams small, each side maps topic names to small integer **keys** in its own sending key space.
- Before using a key, the sender sends `DGRAM_BIND {29: key, 30: name}` on stream 0. `name` is the topic name, optionally with a suffix identifying the instance, e.g. `"cursor:doc-42"`.
- Keys MUST NOT be rebound within a session. Implementations MUST cap bindings per direction (default 256).
- DGRAM_BIND is reliable and precedes use; a datagram with an unbound key MUST be dropped silently (it may have raced ahead of its binding on WebTransport, so this is not an error).

### 11.3 WebTransport encoding
A WebTransport datagram payload is:

```
Datagram {
  Type (8 bits) = 0x40,
  Key  (varint),
  Seq  (varint),
  Data (remaining bytes, session codec)
}
```

- `Seq` starts at 1 per key per direction and increases by 1 per datagram sent for that key.
- Receivers for `latest` topics MUST drop a datagram whose seq is ≤ the highest seq already delivered for that key.
- The total size MUST NOT exceed the smaller of `limits.max_dgram` and the transport's current maximum datagram size (browsers expose it as `maxDatagramSize`). Oversized values are rejected locally; they are never fragmented.

### 11.4 WebSocket emulation
- Datagrams are sent as DATAGRAM frames on stream 0 with header `{29: key, 12: seq}` and the value as data section.
- They are reliable and ordered (TCP), so senders MUST emulate the latency behavior of unreliable delivery:
  - For `latest` topics, a sender keeps at most **one pending unsent datagram per key**, replacing it when a newer value arrives (coalescing).
  - Senders SHOULD send datagram frames with lower priority than other frames, and MAY drop `all`-mode datagrams when their outbound buffer exceeds a threshold.
- The same `max_dgram` limit applies, so applications behave identically on both bindings.

### 11.5 Routing
- Client → server datagrams are delivered to the server's handler for that topic. The server decides whether to fan them out (e.g. relaying a cursor to other subscribers of a document channel).
- Server → client datagrams are produced by server-side targeting (§13).
- Datagrams are **not** sequenced frames: they are never replayed on resumption and never stored in history.

---

## 12. Contract

The contract is the single source of truth for everything typed: server methods, client methods, channels and datagram topics. Implementations derive it from their native definitions (e.g. Rust traits) and export it as a **manifest**. Client generators consume the manifest, never the server's source code.

### 12.1 Type system

| Type | Meaning |
|---|---|
| `unit` | No value |
| `bool` | Boolean |
| `i8` `i16` `i32` `i64` `u8` `u16` `u32` `u64` | Integers |
| `f32` `f64` | IEEE 754 floats |
| `string` | UTF-8 text |
| `bytes` | Byte string |
| `timestamp` | An instant, millisecond precision |
| `uuid` | 128-bit UUID |
| `{"option": T}` | T or absent/null. `{"option": "unit"}` is not allowed |
| `{"list": T}` | Sequence |
| `{"map": T}` | Map with **string** keys and T values |
| `{"tuple": [T...]}` | Fixed-length heterogeneous sequence |
| `{"ref": "Name"}` | Reference to a named type |
| `"any"` | Untyped value (escape hatch; codegen emits `unknown`) |

Named types (in the manifest's `types` table):
- **Struct:** `{"struct": {"fields": [{"name": "...", "type": T, "optional": bool}]}}`. `optional` is only valid on fields whose type is `{"option": T}`; it means the field may be omitted on the wire. A missing key for an `optional` field decodes as absent (`null`); a missing key for any other field is a decode error (INVALID_ARGUMENT).
- **Enum:** `{"enum": {"variants": [{"name": "...", "type": T?}], "open": bool?}}`. A variant without `type` is a unit variant. An **open** enum (`open: true`, default false) tells decoders to map an unknown variant to a generated catch-all (`Unknown` carrying the raw variant name) instead of failing, which makes adding variants a compatible change (§12.7). Application code MUST handle the catch-all. Closed enums stay strict.
- **Alias:** `{"alias": T}`

Generics are not supported in the manifest. Implementations monomorphize each instantiation into a distinct named type: the outer type name, followed for each type argument in order by an underscore and the argument's name as it appears in the type table, or the primitive name (`Page_Message`, `Page_u32`, `Page_List_Message` for a nested instantiation). A collision with a user-defined name is a manifest validation error.

### 12.2 Manifest format
A JSON document:

```json
{
  "pavia": 1,
  "name": "example-app",
  "services": [
    {
      "name": "chat",
      "server_methods": [
        {
          "name": "send",
          "kind": "call",
          "shape": "unary",
          "params": [
            {"name": "room", "type": {"ref": "RoomId"}},
            {"name": "text", "type": "string"}
          ],
          "returns": {"ref": "MessageId"},
          "error": {"ref": "SendError"},
          "idempotent": false
        },
        {
          "name": "typing",
          "kind": "notify",
          "params": [{"name": "room", "type": {"ref": "RoomId"}}]
        },
        {
          "name": "history",
          "kind": "call",
          "shape": "server_stream",
          "params": [{"name": "room", "type": {"ref": "RoomId"}}],
          "yields": {"ref": "Message"}
        },
        {
          "name": "upload",
          "kind": "call",
          "shape": "client_stream",
          "params": [],
          "accepts": "bytes",
          "returns": "u64"
        }
      ],
      "client_methods": [
        {
          "name": "confirm",
          "kind": "call",
          "shape": "unary",
          "params": [{"name": "prompt", "type": "string"}],
          "returns": "bool"
        },
        {
          "name": "systemNotice",
          "kind": "notify",
          "params": [{"name": "text", "type": "string"}]
        }
      ]
    }
  ],
  "channels": [
    {
      "namespace": "room",
      "publication": {"ref": "Message"},
      "presence_info": {"ref": "Member"},
      "history": true
    }
  ],
  "datagrams": [
    {
      "topic": "cursor",
      "direction": "both",
      "mode": "latest",
      "payload": {"ref": "CursorPos"}
    }
  ],
  "types": {
    "RoomId": {"alias": "string"},
    "MessageId": {"alias": "uuid"},
    "Message": {"struct": {"fields": [
      {"name": "id", "type": {"ref": "MessageId"}},
      {"name": "author", "type": "string"},
      {"name": "text", "type": "string"},
      {"name": "sent_at", "type": "timestamp"}
    ]}},
    "SendError": {"enum": {"variants": [
      {"name": "RoomNotFound"},
      {"name": "TooLong", "type": {"struct": {"fields": [{"name": "max", "type": "u32"}]}}}
    ]}},
    "Member": {"struct": {"fields": [{"name": "display_name", "type": "string"}]}},
    "CursorPos": {"struct": {"fields": [
      {"name": "x", "type": "f32"},
      {"name": "y", "type": "f32"}
    ]}}
  }
}
```

Field rules:
- `docs`: any object in the manifest MAY carry a `docs` string (documentation for generators). It is excluded from the fingerprint (§12.5).
- Optional fields (`docs`, `params`, `presence_info`, `snapshot`, `error`, `accepts`, `yields`, `returns`, `idempotent`, `open`, `history`) are omitted when they have no value and MUST NOT be written as `null`. Defaults: `returns` = `unit`, `idempotent` = false, `open` = false, `history` = false; the others default to none.
- `snapshot` (channels): optional type carried in SUBSCRIBED's data section (§10.2).
- `kind`: `call` or `notify`. Notifications have no `shape`, `returns`, `yields`, `accepts` or `error`.
- `shape`: `unary`, `server_stream`, `client_stream`, `bidi` (wire values 0-3).
- `accepts`: item type the caller streams (shapes `client_stream`, `bidi`).
- `yields`: item type the callee streams (shapes `server_stream`, `bidi`).
- `returns`: result type (shapes `unary`, `client_stream`); defaults to `unit`.
- `error`: optional typed error detail (§8.4).
- `idempotent`: whether clients may automatically retry after UNAVAILABLE.
- `direction` for datagrams: `to_server`, `to_client`, `both`.

### 12.3 Codec mapping

| Type | `cbor` | `json` |
|---|---|---|
| `unit` | `null` | `null` |
| `bool` | bool | bool |
| 8-32-bit integers | int | number |
| `i64`, `u64` | int | **string** (decimal), to avoid JavaScript precision loss |
| `f32`, `f64` | float; encoders MAY use half or single precision when the value is exactly representable, decoders MUST accept all three widths | number; NaN/±Infinity are not allowed |
| `string` | text | string |
| `bytes` | byte string | base64url string without padding; decoders reject padded input |
| `timestamp` | tag 1 (epoch seconds); encoders emit an integer for whole seconds and otherwise the shortest float that preserves millisecond precision; decoders accept integer or float | RFC 3339 string in UTC with milliseconds |
| `uuid` | tag 37 with 16-byte byte string | lowercase hyphenated string |
| `option` | value, or `null` | value, or `null` |
| `list`, `tuple` | array | array |
| `map` | map with text keys | object |
| struct | map with text keys = field names; `optional` fields that are absent are omitted | object |
| unit enum variant | text = variant name | string |
| data enum variant | single-entry map `{variant_name: value}` | single-key object |

- Field and variant names on the wire are **exactly** the manifest names. Generators MAY expose idiomatic names in the target language but MUST map them back.
- Decoders MUST ignore unknown struct fields (forward compatibility). Unknown variants of a closed enum are a decode error; open enums map them to the catch-all (§12.1).
- JSON decoders MUST reject NaN/Infinity, duplicate object keys, and invalid UTF-8. CBOR decoders MUST accept only the tags the contract type declares (1 for `timestamp`, 37 for `uuid`); other tags inside typed values are a decode error, while `any` values pass tags through untouched. Data sections are subject to the same depth limit as headers (default 16) and to `max_frame`.

### 12.4 Code generation requirements (TypeScript reference client)
A conforming generator MUST produce:
- Types for every named type. `i64`/`u64` map to `bigint`; `timestamp` to `Date`; `bytes` to `Uint8Array`.
- A typed proxy per service for server methods:
  - unary → `(args..., opts?) => Promise<R>`
  - server stream → `AsyncIterable<Y>`
  - client stream → accepts an `AsyncIterable<A>` and returns `Promise<R>`
  - bidi → accepts an `AsyncIterable<A>` and returns `AsyncIterable<Y>`
  - notify → `(args...) => void`
  - options include `signal: AbortSignal` (→ CANCEL), `deadline`, `meta`.
- A typed registration surface for client methods (handlers for calls and notifications).
- Typed channel subscriptions exposing publications, presence and the recovery position.
- Typed datagram send/receive per topic.
- Typed errors: `ERROR` frames surface as an error class carrying the status code, message and typed detail.
- The manifest fingerprint embedded in the generated code and sent in HELLO.

### 12.5 Fingerprint
The fingerprint is SHA-256 over the manifest serialized with JSON Canonicalization Scheme (RFC 8785) **after removing every `docs` field and the top-level `name`**. Only normative content (services, methods, channels, datagram topics, types) affects it, so editing documentation never invalidates generated clients. It identifies an exact contract version; it says nothing about compatibility. The canonicalization step MUST reject a manifest that contains `null` for an optional field (§12.2).

### 12.6 Publication
Servers MAY serve the manifest at `GET {path}/.well-known/pavia/contract` (JSON, with the fingerprint in an `ETag`). Servers SHOULD allow disabling this in production.

### 12.7 Compatibility rules
| Change | Compatible? |
|---|---|
| Add a service, method, channel namespace or datagram topic | Yes |
| Add an `optional` struct field | Yes |
| Add a required struct field | **No** (old senders omit it) |
| Remove or rename anything | **No** |
| Add a variant to an **open** enum | Yes |
| Add a variant to a closed enum | **No** |
| Change a type, shape or kind | **No** |
| Add a method parameter | **No** |

---

## 13. Server semantics

These rules are not wire format, but conforming server implementations MUST follow them, because clients depend on them.

### 13.1 Targets
Servers MUST support sending notifications (NOTIFY) and datagrams to:

| Target | Recipients |
|---|---|
| All | Every session on the endpoint |
| Session(id) / Sessions(ids) | Specific sessions |
| User(id) / Users(ids) | Every session whose principal has that user ID |
| Group(name) / Groups(names) | Every session in the group(s) |
| Any of the above with Except(session ids) | Minus the listed sessions |

- A session matching a target through several routes receives the message **once**.
- Unknown session IDs, users or groups are silently skipped.
- Sends to detached sessions (§7.8) are buffered for replay; sends to ended sessions are no-ops.
- Server-initiated **calls** (§8.8) target exactly one session. Servers MAY offer a fan-out helper that issues one call per session and aggregates results; on the wire these are independent calls.

### 13.2 Groups
- Add/remove a session to/from a named group; both idempotent. Names are case-sensitive.
- Membership is per session, non-durable, and cleared when the session ends (not when it detaches).
- Group operations MUST work for sessions on other nodes in multi-node deployments.

### 13.3 Sessions and users
- A session has at most one user ID, derived from its principal. Anonymous sessions have none and are never matched by User targets.
- A user may have any number of sessions.

### 13.4 Lifecycle hooks
- **On connect** runs after authentication and before WELCOME is sent. It may reject the session (REJECT with FORBIDDEN or another code). It may join groups.
- **On disconnect** runs exactly once, when the session **ends** (CLOSE, idle timeout without resumption, or `resume_window` expiry), not on transport loss that is later resumed.
- Hooks do not run on resumption.

### 13.5 Backpressure and slow consumers
- Each session has a bounded outbound queue for stream 0 (default 1 MiB), including notifications, channel frames and emulated datagrams.
- Datagrams are dropped first when the queue is over threshold.
- **Lossy-with-recovery for history channels:** when the queue is over threshold, the server SHOULD first drop the session's pending PUBLICATION frames for channels with history and send `UNSUBSCRIBED {code: 2}` for those channels (§10.3); the client resubscribes with `since` and catches up from history at its own pace. Only traffic without a recovery path (notifications, publications on channels without history) keeps the session on the disconnect path below. The replay buffer under resumption follows the same order (§7.8).
- If the queue stays full beyond a configurable grace period (default 5 s), the server MUST close the session with SLOW_CONSUMER (no resumption).
- A slow session MUST NOT delay delivery to other sessions.
- PING/PONG and CLOSE bypass the queue limit.

### 13.6 Fan-out efficiency
A message sent to many sessions SHOULD be encoded once per codec, with only the per-session header fields (`seq`) differing. This is why data sections are separate from headers (§5.2).

### 13.7 Delivery guarantees summary

| Kind | Guarantee |
|---|---|
| Call | Exactly one terminal outcome per call (RESULT/END or ERROR), or local UNAVAILABLE on transport loss |
| Notification | At-most-once; with resumption, exactly-once and ordered per lane within the session lifetime |
| Publication | As notification, plus gapless recovery by `(epoch, offset)` when history is enabled |
| Presence | Eventually consistent; snapshot + deltas |
| Datagram | Best effort; may be lost or reordered (WT); never replayed; `latest` topics deliver monotonically increasing seq |

---

## 14. Security considerations
- **Transport security:** TLS required outside loopback development.
- **Origin validation:** servers MUST check `Origin` on WebSocket upgrades and WebTransport CONNECT requests to prevent cross-site hijacking, especially when cookies authenticate.
- **Credentials:** only in HELLO/AUTH_REFRESH or cookies; never in URLs, never logged. Resume tokens are secrets with the same handling.
- **Unauthenticated exposure:** before WELCOME the server sends nothing but WELCOME, REJECT or CLOSE. Handshake timeout bounds resource use.
- **Resource limits:** servers MUST bound header size, header nesting depth, frame size (`max_frame`), concurrent calls, lanes, datagram bindings, subscriptions, replay buffers and outbound queues, and MUST check lengths before allocating.
- **Parsers:** varint, frame, CBOR and JSON decoders MUST be fuzzed (§17).
- **Channel and method authorization** is per request; authorization at connect time alone is insufficient for channels and methods with finer-grained rules.
- **Datagram floods:** servers SHOULD rate-limit inbound datagrams per session.
- **Error disclosure:** internal error details are not sent to clients by default (§8.4).
- **Presence privacy:** presence reveals who is subscribed; channel authorization must account for it. Session IDs appear in presence entries; they are public identifiers, never credentials (§7.2), and cannot be used to target or resume a session from the client side.
- **Rate limits (normative):** per source address: handshakes and resume attempts. Per session: stream opens and CANCELs (§6.5), PINGs (§7.4), notifications, datagrams, control requests. Defaults in §16. Exceeding a per-session limit closes with LIMIT_EXCEEDED; per-address limits reject at the transport layer.
- **Replay:** no frame from TLS/QUIC 0-RTT early data is processed (§4.1); resume tokens are single-use (§7.8).
- **Compression:** none in v1 and `permessage-deflate` is not negotiated (§4.3). Any future compression MUST never compress credential-bearing frames (HELLO, AUTH_REFRESH) together with attacker-influenced data.
- **CBOR:** headers contain no tags and MUST be rejected if they do; typed data accepts only declared tags (§12.3). Depth, size and length checks precede allocation everywhere.

---

## 15. Registries

### 15.1 Session close codes
Used by REJECT, GOAWAY and CLOSE. On WebSocket, the close code is `4000 + code`.

| Code | Name | Meaning |
|---|---|---|
| 0 | NORMAL | Normal closure |
| 1 | PROTOCOL_ERROR | Malformed or disallowed frame |
| 2 | UNSUPPORTED_VERSION | No common protocol version or codec |
| 3 | UNAUTHENTICATED | Missing, invalid or expired credentials |
| 4 | FORBIDDEN | Authenticated but not allowed |
| 5 | IDLE_TIMEOUT | Nothing received for `idle` ms |
| 6 | SLOW_CONSUMER | Outbound queue full too long |
| 7 | GOING_AWAY | Server draining |
| 8 | INTERNAL | Unexpected server failure |
| 9 | FRAME_TOO_LARGE | Frame exceeds `max_frame` |
| 10 | LIMIT_EXCEEDED | Server capacity or a session limit exceeded |
| 11 | RESUME_FAILED | Resumption not possible |
| 12 | HANDSHAKE_TIMEOUT | No HELLO in time |
| 13 | CONTRACT_MISMATCH | Strict contract policy rejected the client |

### 15.2 Call status codes
Used by ERROR, REQUEST_ERROR and AUTH_RESULT. Values and meanings follow gRPC status codes so that tooling and intuition transfer.

| Code | Name | Typical use |
|---|---|---|
| 0 | OK | Success (never in ERROR) |
| 1 | CANCELLED | Caller cancelled |
| 2 | UNKNOWN | Unknown error |
| 3 | INVALID_ARGUMENT | Bad arguments, shape mismatch, invalid channel name |
| 4 | DEADLINE_EXCEEDED | Deadline expired |
| 5 | NOT_FOUND | Application-level "not found" |
| 6 | ALREADY_EXISTS | Duplicate subscription, application conflicts |
| 7 | PERMISSION_DENIED | Authorization failed |
| 8 | RESOURCE_EXHAUSTED | Limits (concurrency, subscriptions, sizes) |
| 9 | FAILED_PRECONDITION | Application precondition not met |
| 10 | ABORTED | Concurrency conflict |
| 11 | OUT_OF_RANGE | Application range errors |
| 12 | UNIMPLEMENTED | Unknown method |
| 13 | INTERNAL | Unexpected failure |
| 14 | UNAVAILABLE | Draining server, transport loss |
| 15 | DATA_LOSS | Unrecoverable data loss |
| 16 | UNAUTHENTICATED | Session has no valid credentials for this operation |

### 15.3 Unsubscribe reasons
`0` normal, `1` permission revoked, `2` resubscribe required, `3` channel closed.

### 15.4 Presence kinds
`0` snapshot, `1` join, `2` leave, `3` update.

---

## 16. Defaults

| Parameter | Default | Where |
|---|---|---|
| Handshake timeout | 10 s | §7.1 |
| Heartbeat `hb` | 15 s | §7.4 |
| Idle timeout `idle` | 45 s | §7.4 |
| `max_frame` | 1 MiB | §5, §7.2 |
| Header size limit | 16 KiB | §5.4.1 |
| Header nesting depth | 16 | §5.4.1 |
| Initial stream credit `window` (WS) | 256 KiB | §6.4 |
| `max_calls` (each direction) | 100 | §6.5 |
| `max_subs` | 256 | §10.2 |
| `max_dgram` | 1,100 bytes | §11.3 |
| Datagram bindings per direction | 256 | §11.2 |
| Lanes per direction | 16 | §9.2 |
| `resume_window` | 30 s | §7.8 |
| Replay buffer (per direction) | 1 MiB | §7.8 |
| ACK interval | 1 s or 64 frames | §7.8 |
| Outbound queue (stream 0) | 1 MiB | §13.5 |
| Slow-consumer grace | 5 s | §13.5 |
| Pre-authentication buffer | 64 KiB | §4.1 |
| WebSocket message size | 2 × `max_frame` | §4.3 |
| Client send high-water mark (WS) | 1 MiB | §4.3 |
| Server inbound unprocessed bytes | 1 MiB | §4.3 |
| Stream open + CANCEL rate | 1,000 / 10 s | §6.5 |
| PING rate | 10 / `hb` | §7.4 |
| Notification rate (inbound) | 500 / s | §14 |
| Datagram rate (inbound) | 2,000 / s | §14 |
| Control request rate | 100 / s | §14 |
| Handshake + resume attempts per address | 60 / min | §14 |
| Detached sessions per principal | 8 | §7.8 |
| Maximum session age | 4 h ± 10 % | §7.6 |
| Idempotency retention | 5 min | §8.7 |
| Transport race head-start | 250 ms | §4.4 |
| WebTransport failure cache | 24 h (1 h when WELCOME lists `webtransport`) | §4.4 |

---

## 17. Conformance

### 17.1 Conformance levels
| Level | Includes |
|---|---|
| **Core** | WebSocket binding, framing, session (§7.1-7.7, 7.9), unary calls, notifications, `cbor` codec |
| **Streaming** | Core + stream shapes 1-3, CANCEL, deadlines, WS credit |
| **Server calls** | Core + §8.8 |
| **WebTransport** | Core over the WebTransport binding, lanes |
| **Datagrams** | §11 on each supported binding |
| **Resumption** | §7.8 |
| **Channels** | §10.1-10.4 |
| **History** | §10.5 |
| **Presence** | §10.6 |

An implementation states which levels it supports. Clients and servers negotiate the rest via capabilities.

### 17.2 Test vectors
The project maintains machine-readable test vectors (`vectors/`):
- **Frame vectors:** hex bytes ↔ decoded frame (type, flags, header in diagnostic notation, data in diagnostic notation), for both bindings. Includes invalid vectors with the expected error.
- **Codec vectors:** contract type + value ↔ `cbor` hex ↔ `json` text.
- **Scripts:** ordered exchanges (`send`/`expect`/`expect_nothing_within`/`close_transport`) describing a scenario from the client's perspective. A server conformance runner replays scripts against a server under test; a client conformance runner plays the server side against a client under test.

- **Abuse scripts:** scripts that exercise every limit in §16 (rapid reset, ping flood, handshake flood, slow HELLO, oversized varints, CBOR depth bombs, datagram floods) and assert the specified close code within a bounded time.

Every normative MUST in this document should be covered by at least one vector or script. Changes to this specification MUST update vectors in the same change.

---

## 18. Open questions
1. **Compression:** per-frame zstd/deflate with a shared-dictionary scheme that never mixes credentials and untrusted data. `permessage-deflate` is excluded (§4.3). Flag bit 0 is reserved. **Decision (draft 0.3):** deferred. Flag bit 0 stays reserved; nothing else is reserved.
2. **Method name interning:** strings in every CALL/NOTIFY are simple but wasteful at high rates. A binding table (like DGRAM_BIND) is a candidate for v1.1; measure first, since CBOR short strings are cheap. **Decision (draft 0.3):** deferred to v1.1. The addition is compatible: a METHOD_BIND extension frame (type 0x80, reserved in §5.6) plus a new header key, negotiated by a capability.
2a. **`cbor-packed` codec (v1.1):** encode structs as arrays in manifest field order, allowed only when both fingerprints match (§12.5). Removes field names from every payload; the fingerprint already guarantees agreement.
2b. **Delta publications (post-1.0):** for large, frequently updated channel values, publish diffs against the previous offset (as Centrifugo does), with a full value at recovery boundaries.
3. **Client-published channel messages** with server-side authorization, versus always going through a server method.
4. **Invisible channel members** (subscribe without appearing in presence).
5. **Generics** in the type system. **Decision (draft 0.3):** not in the manifest; the monomorphization naming rule in §12.1 is normative.
6. **WebTransport over HTTP/2** (IETF draft) as a third binding once browsers ship it; it would replace WebSocket where UDP is blocked.
7. **Session-level flow control on WebSocket** in addition to per-stream credit. **Decision (draft 0.3):** deferred. In-flight ITEM bytes are bounded by `max_calls × window` (§6.4), stream 0 by the outbound queue (§13.5) and the inbound limit (§4.3).
8. **Priorities** between call streams. **Decision (draft 0.3):** deferred. v1 uses round-robin across call streams (§6.6) and the QUIC stack's defaults; a per-call priority can be added later as a new header key under a capability.
9. **Resumable calls** (surviving transport loss), currently deliberately excluded for simplicity.

---

## Appendix A. Example exchange (WebSocket, resumption granted)

```
C→S  sid=0  HELLO     {14: [1], 16: ["cbor","json"], 18: ["calls","streams","resume","channels","history"],
                       24: h'9f86...', 33: {35: "bearer", 13: "eyJ..."}, 34: {30: "pavia-ts", 52: "0.1.0"}}
S→C  sid=0  WELCOME   {15: 1, 17: "cbor", 18: ["calls","streams","resume","channels","history"],
                       19: "s_7Qm2", 13: h'a1...', 21: {40: 262144, 48: 100, 49: 1048576, 50: 256, 51: 1100},
                       22: 15000, 23: 45000, 37: 30000, 24: h'9f86...'}
C→S  sid=0  SUBSCRIBE {1: 1, 9: "room:general", 41: {10: "e1", 11: 41}, 12: 1}     ← pipelined before WELCOME arrives
S→C  sid=0  SUBSCRIBED{1: 1, 9: "room:general", 10: "e1", 11: 43, 26: true, 12: 1}
S→C  sid=0  PUBLICATION {9: "room:general", 11: 42, 12: 2}  data: {"id": 37(h'...'), "author": "ana", "text": "hi", "sent_at": 1(1758499200.123)}
S→C  sid=0  PUBLICATION {9: "room:general", 11: 43, 12: 3}  data: {...}
C→S  sid=2  CALL      {2: "chat/send", 5: 0, 4: 5000}       data: ["general", "hello"]
S→C  sid=2  RESULT    {44: {0: 1}}                            data: 37(h'...')        ← ack piggybacked
S→C  sid=0  PUBLICATION {9: "room:general", 11: 44, 12: 4}  data: {...}
C→S  sid=0  ACK       {44: {0: 4}}
C→S  sid=4  CALL      {2: "chat/history", 5: 1}              data: ["general"]
S→C  sid=4  ITEM      {}                                     data: {...}
S→C  sid=4  ITEM      {}                                     data: {...}
C→S  sid=4  CANCEL    {}
S→C  sid=4  ERROR     {6: 1, 7: "cancelled"}
             ... transport drops ...
C→S  sid=0  HELLO     {14: [1], 16: ["cbor"], 18: [...], 20: {13: h'a1...', 44: {0: 4}}, 33: {...}}
S→C  sid=0  WELCOME   {..., 19: "s_7Qm2", 13: h'b7...', 43: true, 44: {0: 1}}
S→C  sid=0  PUBLICATION {9: "room:general", 11: 45, 12: 5}  data: {...}   ← buffered while detached
```

---

## Changelog

### draft 0.5
- Replay buffer pressure: recoverable-first eviction instead of backpressure or loss of resumability; gaps reported per lane with the new key 53 `lost` in WELCOME and in the `resume` map; receivers re-anchor on listed lanes (§5.7, §7.1, §7.2, §7.8, §13.5). Decision #152.

### draft 0.4
- `limits` and `client` maps use integer keys; new keys 48 to 52; key 40 is `window` inside `limits` (§5.7, §7.1, §7.2).

### draft 0.3
Editorial and consistency pass from the C0.1 review (issues #229 to #248). No new features.
- `max_frame` defined as a bound on the Length field (§5.2).
- The `window >= max_frame` rule is replaced by a credit-grant obligation; total in-flight bound stated (§6.4).
- Key 20 `resume` is `{13: token, 44: ack}` (§5.7).
- Lanes on WebSocket use lane streams with LANE_OPEN like WebTransport; the `lane` key on NOTIFY is gone (§4.3, §5.7, §9.1, §9.2).
- AUTH_RESULT uses call status codes; malformed AUTH_REFRESH gets REQUEST_ERROR (§7.5).
- `auth` map uses integer keys; new key 35 `scheme` (§5.7, §7.1).
- Server `window` moves into WELCOME `limits`; key 40 unassigned (§7.2).
- Concurrency check is the first callee check (§8.2).
- Every channel carries `epoch` and `offset`; history only bounds `since` (§10.2, §10.4, §10.5).
- Manifest optional fields are absent, never `null`; defaults listed; canonicalization rejects `null` (§12.2, §12.5).
- GOAWAY `last_stream` removed; key 39 unassigned (§5.7, §7.6).
- 426 only on HTTP/1.1; 405 on HTTP/2 and HTTP/3 (§4.3).
- Control stream FIN or reset without CLOSE is a transport loss (§6.2).
- Registry gaps 39, 40, 42 marked unassigned (§5.7).
- Codec rules pinned: `optional` semantics, `option<unit>` forbidden, timestamp int or float, float widths, base64url padding (§12.1, §12.3).
- Generics: normative monomorphization naming (§12.1).
- Section 18 items 1, 2, 5, 7 and 8 carry explicit decisions; type 0x80 reserved for METHOD_BIND (§5.6, §18).

### draft 0.2
- Pre-WELCOME pipelining with a pre-auth buffer bound; small QUIC windows before authentication (§4.1, §4.2, §6.2).
- 0-RTT early data never processed; `permessage-deflate` excluded; `426` for non-upgrade requests; WebSocket message size cap; client `bufferedAmount` backpressure; server inbound backpressure with idle-timer pause (§4.1, §4.3).
- `transports` hint in WELCOME and a shorter failure cache (§4.4).
- Client `window`, `max_frame`, `max_dgram` in HELLO; each side's limits bound frames sent to it (§6.4, §7.1).
- Stream open/cancel rate limit; sender scheduling classes and coalescing (§6.5, §6.6).
- Session ID format and uniqueness; session IDs are never credentials (§7.2, §14).
- Server-driven liveness for throttled browser tabs; PING rate limit (§7.4).
- Authorization re-evaluation on refresh and resume (§7.5).
- Staggered draining and maximum session age (§7.6).
- Resumption: per-lane sequence counters; all control-stream frames sequenced; `ack` maps, piggybackable; codec/version fixed on resume; detached-session calls fail with `nx`; detached-session limits (§7.8).
- `nx` flag on ERROR and `pavia-idempotency-key` metadata with capability `idempotency` (§8.4, §8.7, §7.10).
- CANCEL/terminal race defined (§8.5).
- Snapshot-on-subscribe (§10.2, §12.2); single-sequencer constraint for history (§10.5); presence `update` kind (§10.6).
- Open enums; `docs` fields excluded from the fingerprint; JSON and CBOR decoder strictness (§12.1, §12.3, §12.5, §12.7).
- Lossy-with-recovery for history channels before SLOW_CONSUMER (§13.5).
- Normative rate limits and abuse scripts (§14, §16, §17.2).
- New open questions: `cbor-packed`, delta publications (§18).
