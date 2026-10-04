# Spec review: issue consistency pass (draft 0.8)

Status: accepted, 2026-10-04.

A cross-issue consistency review of all open issues found contradictions between issues and the spec, and gaps where the spec gave no outcome. This record lists the protocol decisions taken for draft 0.8. Section numbers refer to `spec/pavia-protocol.md`.

## Wire and streams

### Request half after an early terminal frame
6.3 lets the callee send RESULT or ERROR before the request half closes, but no frame then closes that half, so the callee cannot tell when to free the stream entry.
Rule: the early RESULT or ERROR completes the call; the request half is closed implicitly and frames already in flight are handled as frames on a closed stream (6.1). On WebTransport the callee sends STOP_SENDING.
Rejected: the caller closes the half with END or FIN, because the callee then needs a timer for callers that never close; the caller sends CANCEL, because it costs the same round trip and gives CANCEL a second meaning.

### Undecodable data sections outside CALL and NOTIFY
Only CALL and NOTIFY define a decode failure; an undecodable ITEM, RESULT, ERROR detail, PUBLICATION, PRESENCE entry, snapshot or DATAGRAM value had no outcome.
Rule: failures stay per call or per message. A callee answers an undecodable ITEM with ERROR INVALID_ARGUMENT; a caller completes the call locally with INTERNAL and sends CANCEL if the call is open; other values are dropped and reported as a local error event.
Rejected: PROTOCOL_ERROR for every undecodable section, because under the default `warn` policy (7.2) a decode failure is usually contract skew, and one bad method would disconnect the whole session.

### An ITEM larger than max_frame
6.4 rejects such an item locally but does not say whether the call continues.
Rule: only the producing code gets RESOURCE_EXHAUSTED; nothing is sent and the call stays open.
Rejected: the sender terminates the call, because the oversized item is a local error in one value and only the producer can recover from it.

### FIN before the terminal frame on a WebTransport call stream
5.3 covers a FIN in the middle of a frame, but a FIN at a frame boundary before END, RESULT or ERROR had no meaning.
Rule: it is handled like RESET_STREAM on that half (caller FIN counts as CANCEL, callee FIN as ERROR UNAVAILABLE).
Rejected: PROTOCOL_ERROR, because one broken call would end every call; an implicit END, because a crash mid-upload would look like a finished upload.

### Rate-limit windows
6.5 and 16 give rates without defining the window, and abuse scripts must assert an exact threshold.
Rule: every rate is a token bucket whose capacity is the stated count and whose refill is the count divided by the period. Frames retransmitted after resumption do not count. CALL and LANE_OPEN both count as stream opens.
Rejected: a sliding window, because it needs a timestamp ring per limit; a fixed window, because it allows twice the rate across a window edge.

### The stream-0 outbound queue over its bound
13.5 speaks of a threshold and of a full queue without defining either, and does not say what happens to a notification that does not fit during the grace period.
Rule: the threshold is the bound. Over it, datagrams are dropped and history publications are replaced by UNSUBSCRIBED code 2; other frames queue up to a hard cap of 2 x the bound. Reaching the hard cap, or staying over the bound past the grace period, closes with SLOW_CONSUMER.
Rejected: dropping notifications that do not fit, because it breaks the exactly-once promise of 9.1 silently; closing as soon as one does not fit, because short bursts would then disconnect sessions.

## Session and handshake

### Limits a client honors before WELCOME
6.4 bounds frames by the peer's advertised limits, but a pipelining client sends frames before it has seen them.
Rule: a client sends at most 64 KiB of frames after HELLO before WELCOME. Servers accept at least 64 KiB and never advertise `max_frame` or `window` below 64 KiB.
Rejected: assuming the 16 defaults until WELCOME, because a server configured below them closes clients that follow the rule.

### Limits a client advertises in HELLO
The 64 KiB floor on `max_frame` and `window` bound only what servers advertise, so a client could still announce a smaller limit and make every server frame toward it fail.
Rule: the floor binds both peers. A HELLO advertising `max_frame` or `window` below 64 KiB is malformed and handled as a PROTOCOL_ERROR like any other malformed frame (7.1).
Rejected: binding servers only, because a server would then need a fallback for frames it cannot fit under the client's limit.

### Limits and timers missing from 16
7.6 and 7.8 require a drain window, a drain deadline and a total cap on detached sessions, but 16 gave no defaults.
Rule: 16 gains rows for every value a client can observe: drain window 60 s, drain deadline 5 min, detached sessions in total 10,000 per node. Server-only capacity knobs stay implementation defaults.
Rejected: keeping 16 as is, because conformance scripts could not assert drain timing.

### Refusal statuses and requests without Origin
No HTTP status was given for refused upgrades, and nothing covered requests without an `Origin` header.
Rule: a request without `Origin` is not subject to the allow-list. Refusals are 400 for a missing `pavia.1`, 403 for a disallowed Origin and 429 with `Retry-After` for a per-address rate limit, on every HTTP version and on the WebTransport CONNECT response.
Rejected: refusing requests without Origin unless configured, because cross-site hijacking needs a browser and browsers always send Origin; accepting them only without cookies, because a client holding stolen cookies can send them anyway.

### What the handshake timeout covers
7.1 bounds only the arrival of HELLO, leaving the authenticator and the on-connect hook unbounded while pipelined frames sit in the pre-authentication buffer.
Rule: one timer runs from transport establishment until WELCOME or REJECT is sent (default 10 s), ending in HANDSHAKE_TIMEOUT.
Rejected: a separate authentication timeout, because it adds a timer and a close code for little gain; leaving the time after HELLO unbounded, because a hanging authenticator would hold pre-authentication buffers forever.

### Group joins made by the on-connect hook
13.4 lets on-connect join groups before WELCOME, while 4.1 forbids sending anything but WELCOME, REJECT or CLOSE before WELCOME.
Rule: joins are staged and take effect atomically with registration when WELCOME is sent; on REJECT they are discarded. Sends issued before WELCOME do not reach the session.
Rejected: registering before on-connect and queueing sends behind WELCOME, because it adds a queue for sessions that may still be rejected; returning the groups from the hook, because joins could no longer depend on other hook effects.

### Contract fingerprint missing on one side
HELLO `contract` is only SHOULD, so `strict` had no rule for a HELLO without it or for a server without a manifest.
Rule: under `strict` a HELLO without `contract` is a mismatch; under `warn` it is logged. A server without a manifest omits key 24 and never rejects on fingerprints.
Rejected: treating a missing fingerprint as never a mismatch, because any client could bypass `strict` by omitting it.

### Pipelined frames during the transport race
4.4 races two bindings, and a CALL pipelined on both attempts can run twice if both reach WELCOME.
Rule: while more than one attempt is in flight a client sends only HELLO; it pipelines only on an attempt that is alone.
Rejected: pipelining on the first attempt and resending safe frames on the winner, because it needs a per-frame safety classification that is easy to get wrong.

### Client jitter around retry
7.6 asks for jitter of plus or minus 20 % without saying whether a client may return before `retry`.
Rule: `retry` is a floor; clients wait `retry` x U(1.0, 1.2).
Rejected: `retry` x U(0.8, 1.2), because a server at capacity could not rely on `retry` as a quiet period.

## Calls and idempotency

### Idempotency key without the idempotency capability
8.4 lets keyed calls retry, 8.7 deduplicates only with capability `idempotency`, and 7.10 makes ungranted features a PROTOCOL_ERROR.
Rule: without the capability the key is ordinary metadata and not an error; clients count it toward automatic retry only when the capability was granted.
Rejected: PROTOCOL_ERROR, because it breaks calls pipelined before WELCOME and rolling deployments; client policy, because a retry against a server that ignores the key duplicates side effects.

### Idempotency key scope and stored outcomes
8.7 did not say which outcomes are stored, how anonymous sessions and other methods scope a key, or what a duplicate gets while the first call runs.
Rule: entries are keyed by (principal, or session ID for anonymous sessions; method; key). Only RESULT and handler ERRORs are stored. An in-flight duplicate gets ERROR ABORTED with `nx: true`. A key over 128 bytes gets INVALID_ARGUMENT at check 4.
Rejected: waiting for the first outcome, because it needs a claim marker and a wait path across nodes; storing every terminal outcome, because a retry after a cancel or timeout would return that failure for the whole retention window.

### Invalid metadata names
8.7 gives a name grammar but no outcome for a violation and no rule for unknown `pavia-*` names.
Rule: a name outside the grammar makes the header malformed (PROTOCOL_ERROR, 5.4.1). Receivers ignore `pavia-*` names they do not implement.
Rejected: per-request INVALID_ARGUMENT, because `meta` appears on six frame types and each would need its own error path.

### When an idempotent call may be retried
12.2 ties `idempotent` to retry after UNAVAILABLE, 8.4 names no status, and 7.8 does not say whether a retry may use a new session.
Rule: `idempotent` means running the call more than once has the same effect. Clients may retry after any failure with an unknown outcome, on the same, a resumed or a new session; which statuses trigger a retry is client policy.
Rejected: restricting retries to UNAVAILABLE and transport loss in the protocol, because that is a policy choice for the application; allowing them only after `resumed: true`, because the method is safe on any session.

### Server-initiated call to an unknown or ended session
13.1 skips unknown targets silently, which fits sends but not calls.
Rule: the call fails locally with NOT_FOUND and `nx: true`; a detached session keeps UNAVAILABLE with `nx: true`.
Rejected: UNAVAILABLE for both, because the caller could not tell a session that may return from one that is gone.

## Contract and codegen

### Manifest fields equal to their default
12.2 says defaults exist, yet its example writes `"idempotent": false`, and writing a default changes the fingerprint.
Rule: optional fields equal to their default MUST be omitted; canonicalization rejects them like `null`.
Rejected: allowing defaults and stripping them before hashing, because every fingerprint implementation would need the default table.

### Empty params list
12.2 lists `params` as optional with no default value, so `"params": []` and an omitted `params` described the same method with two fingerprints.
Rule: an empty `params` list equals the default and MUST be omitted; canonicalization rejects it.
Rejected: always writing `params`, because it makes `params` the only optional field with a mandatory presence rule.

### Order of named entries in the manifest
The manifest did not say how arrays are ordered, so reordering services or methods in the source changed the fingerprint without changing the contract.
Rule: arrays of named entries whose position carries no meaning (`services`, `server_methods`, `client_methods`, `channels`, `datagrams` and later lists of the same kind) are sorted by name in byte order. `params`, struct `fields` and enum `variants` keep declaration order, because position matters for encoding. Exporters sort; canonicalization rejects unsorted input. The 12.2 example is reordered to comply.
Rejected: declaration order everywhere, because reordering source declarations would change the fingerprint.

### One grammar for manifest names
8.1 gives a grammar only for services and methods.
Rule: service, method, type, field, variant and datagram topic names match `[A-Za-z_][A-Za-z0-9_]*`; channel namespaces use the 10.1 characters without `:`.
Rejected: any UTF-8 for fields and variants, because generators would have to quote and escape names, and compatibility with other wire formats is a non-goal (1.2).

### Open-enum catch-all
12.1 maps unknown variants to `Unknown` but says nothing about a declared variant with that name or about encoding the catch-all.
Rule: `Unknown` is reserved in open enums (manifest validation error); the catch-all is decode-only and encoding it is a local error.
Rejected: picking a non-colliding name and re-encoding as a unit variant, because the payload was dropped on decode and the re-encoded value would differ from the one received.

### Well-known contract endpoint
12.6 defines neither the `ETag` encoding nor the response when publication is disabled.
Rule: the `ETag` is the lowercase hex fingerprint as a strong tag; a disabled endpoint answers 404 on every HTTP version.
Rejected: falling back to the 426/405 rules with a base64url tag, because the 4.3 exemption would become conditional and hex is what vectors and logs print.

### Compatibility verdicts for unlisted changes
The 12.7 table did not cover changes such as flipping `idempotent`, toggling `open`, or adding `snapshot`, `presence_info` or `error`.
Rule: a conservative table. Compatible: `docs` or `name` only, closed to open enum, `idempotent` false to true, enabling `history`. Breaking: the reverse flips, any datagram `mode` or `direction` change, adding `snapshot`, `presence_info`, `subscribe` or `error`, and any change not listed.
Rejected: a permissive table, because it needs a new rule that receivers ignore data sections they have no type for.

## WebTransport, lanes and datagrams

### WebTransport sessions per QUIC connection
4.2 applies connection-level QUIC limits to one Pavia session, but a browser may pool two sessions on one connection.
Rule: a server accepts at most one WebTransport session per QUIC connection and advertises that limit. Bidirectional MAX_STREAMS is `max_calls` plus one for the control stream.
Rejected: the per-session flow-control capsules of the WebTransport draft, because browser support is uneven; per-connection bounds, because one session could starve another.

### Limit for server-initiated calls on WebTransport
6.5 called the client's `max_calls` informational on WebTransport, but a browser client cannot set its QUIC MAX_STREAMS.
Rule: the server enforces the client's `max_calls` on both bindings; beyond it a server-initiated call fails locally with RESOURCE_EXHAUSTED and `nx: true`.
Rejected: the minimum of `max_calls` and the client's MAX_STREAMS, because calls would wait on browser internals and hide overload.

### QUIC idle timeout shorter than Pavia idle
The effective QUIC idle timeout is the minimum of both endpoints' values, so the server alone could not meet the 4.2 MUST.
Rule: when the client's value leaves less than 15 s of margin, the server lowers WELCOME `idle` (and `hb`) to 15 s below the effective QUIC timeout, or to half of it when the effective timeout is 30 s or less, so `idle` never reaches zero.
Rejected: documenting the gap, because QUIC would then close first and the session would see transport loss instead of IDLE_TIMEOUT.

### Malformed or oversized received datagrams
11.3 states only sender rules.
Rule: a WebTransport datagram that fails to decode or exceeds the receiver's `max_dgram` is dropped. A WebSocket DATAGRAM frame over `max_dgram` is a PROTOCOL_ERROR, because it came through the reliable framed path.
Rejected: PROTOCOL_ERROR on both bindings, because ending a session over one corrupt datagram is out of proportion for a lossy channel.

### Notification handler ordering within a lane
9.2 orders delivery within a lane but not handler execution.
Rule: handlers on one lane run one at a time in seq order; the next starts after the previous one completes.
Rejected: ordered dispatch with concurrent execution, because effects could interleave and the ordering would give applications little. Lanes remain the way to get parallelism.

### Lane and datagram-binding caps
The caps in 9.2 and 11.2 are not advertised, so a sender cannot know them, and 11.2 names no close code.
Rule: fixed protocol values. Lanes are numbered 1 to 16; at most 256 binding keys per direction. LANE_OPEN for lane 0 or above 16, a second LANE_OPEN for an open lane, a rebind of a bound key and a 257th key are PROTOCOL_ERROR.
Rejected: advertising both caps in `limits`, because it adds keys and negotiation for values few deployments change; private caps, because senders could not refuse locally.

### DGRAM_BIND names and binds for unusable topics
11.2 gives an example, not a syntax, and no outcome for an undeclared topic or a forbidden direction.
Rule: the topic is the part of the name before the first `:`; names use the 10.1 characters and are at most 255 bytes. A bind for an undeclared topic or a forbidden direction is ignored and its key stays unbound.
Rejected: PROTOCOL_ERROR for such binds, because datagrams are best effort and contract skew is tolerated by default.

### Datagram seq across resumption
7.8 keeps datagram bindings across resumption, but a sender restarting at seq 1 would have every value dropped by a receiver that kept its position.
Rule: on resumption both peers reset datagram seq; senders restart each key at 1 and receivers clear their highest-delivered values. Only the key-to-name binding survives.
Rejected: carrying counters across resumption and takeover, because datagrams are never replayed and the extra state would burden the handover.

### Inbound datagram floods
14 had both a SHOULD bullet for datagram floods and a normative rate limit closing with LIMIT_EXCEEDED.
Rule: inbound datagrams MUST be rate-limited and exceeding the limit closes with LIMIT_EXCEEDED. Every inbound datagram counts, including those dropped as unbound or malformed.
Rejected: dropping excess and closing only above a hard limit, because it adds a second threshold to test; a configurable choice, because abuse scripts could not pin the outcome.

### FIN or reset of a lane stream
9.2 calls a reset lane stream a "transport error", a term defined nowhere, and says nothing about FIN.
Rule: a FIN or reset of a lane stream is transport loss (7.7), as for the control stream (6.2); the session detaches if resumption was granted.
Rejected: PROTOCOL_ERROR, because a single stream problem would lose all session state.

## Resumption

### Seq holes from eviction and queue drops
7.8 recorded a gap only for one eviction class, and 13.5 dropped queued publications without saying whether they had consumed a seq.
Rule: seq is assigned when a frame is first written to a transport. Evicting a recoverable frame (history PUBLICATION or PRESENCE) that was never sent leaves no gap, since the resubscribe path covers it; evicting a sent, unacknowledged recoverable frame lists its lane in `lost`. Every other eviction, sent or unsent, lists its lane in `lost`, so a dropped notification always reaches the application as a gap event. A missing seq, or a skip on a lane not listed in `lost`, is a PROTOCOL_ERROR.
Rejected: listing the lane in `lost` on every eviction plus a new key for recoverable-only loss, because it adds a key and re-anchoring on every eviction; the same without the key, because every evicted publication would raise a gap and defeat the recoverable-first order.

### Reopening lane streams after resumption
LANE_OPEN was sequenced with seq 1, so once acknowledged it left the replay buffer and a resent copy would be discarded as a duplicate.
Rule: LANE_OPEN is not sequenced. It is sent first on every lane stream, including reopened ones, and the first NOTIFY on a lane has seq 1.
Rejected: pinning LANE_OPEN in the buffer, because it adds exceptions to trimming, eviction and duplicate discard; sending it lazily on next use, because replayed frames would have no stream to travel on.

### A sequenced frame larger than the replay buffer
The default replay buffer equals the default `max_frame`, so one large sequenced frame could not fit even after evicting everything.
Rule: a sequenced frame that would not fit in an empty replay buffer is rejected locally with RESOURCE_EXHAUSTED and never sent.
Rejected: a buffer floor of 2 x the peer's `max_frame`, because it doubles memory per detached session; sending it unbuffered, because a reliable frame type would silently lose that property.

### Client control requests under buffer pressure
Control requests are sequenced so a lost SUBSCRIBE cannot leave the client waiting, but eviction could still drop them.
Rule: control requests are never evicted. A new one that does not fit after all evictable frames are gone is rejected locally with RESOURCE_EXHAUSTED.
Rejected: evicting and failing them locally, or reporting them in `lost`, because the server may still answer a request the client gave up on and the client would need a reconciliation path.

### Replies and DGRAM_BIND under buffer pressure
Control requests were protected from eviction, but their replies and DGRAM_BIND were not: an evicted SUBSCRIBED, REQUEST_ERROR or AUTH_RESULT left a `rid` without an answer, and an evicted DGRAM_BIND left a key unbound for the rest of the session, since keys are never rebound.
Rule: SUBSCRIBED, REQUEST_ERROR, AUTH_RESULT, an UNSUBSCRIBED that answers an UNSUBSCRIBE, and DGRAM_BIND in both directions are never evicted, like control requests. A frame that still cannot fit is rejected locally with RESOURCE_EXHAUSTED under the existing rule.
Rejected: evicting them and failing the `rid` at resume, because it needs per-`rid` tracking of evicted replies and an exception to the no-rebind rule for keys.

### Locating a session on resume
A node that does not own the session cannot tell which session a resume is for; the `?session=` parameter is only a routing hint.
Rule: the `resume` map carries the session ID (key 19). A node that does not own the session finds the owner by it. The token remains the only proof.
Rejected: requiring `?session=` for lookup, because proxies may rewrite query strings; a self-locating token, because it makes the token format normative; a directory keyed by token hash, because tokens rotate on every WELCOME.

### Seq on pipelined frames and early replay
The Appendix A example pipelines a sequenced SUBSCRIBE before the client knows whether `resume` is granted, and 7.8 did not say whether replay may start before WELCOME.
Rule: pipelined frames carry `seq` when the client requested `resume`; a server that does not grant it ignores `seq`. A resuming client may replay from its last known acknowledgment before WELCOME; the server discards duplicates.
Rejected: waiting for WELCOME, because it adds a round trip on every connect and resume.

### Capabilities on a resuming HELLO
7.8 fixes version and codec on resume but says nothing about `caps`.
Rule: a resuming HELLO requests at least the original grant and the resumed WELCOME grants exactly that set; otherwise RESUME_FAILED.
Rejected: renegotiating on resume, because every feature would need a teardown path for state that depends on a dropped capability.

### Detached-session caps
Anonymous sessions have no principal for the per-principal cap, and the scope of the caps was not stated.
Rule: caps apply per node; anonymous sessions are grouped by source address.
Rejected: exempting anonymous sessions, because one address could fill the total cap; cluster-wide caps, because they need a shared counter and a backplane round trip on every detach.

### Failed resume attempts and principal equality
7.8 did not say whether a failed attempt spends the token, and 7.5 did not define principal equality.
Rule: only a successful resume spends the token. Principals are equal when the authenticator's stable principal ID is equal (the user ID by default); an anonymous session resumes only from an anonymous HELLO.
Rejected: spending the token on any attempt, because a token thief could then end the victim's detached session.

## Auth

### Authorizing client notifications
8.2 and 10.2 authorize CALL and SUBSCRIBE, but nothing covered an inbound NOTIFY, which runs a handler with side effects.
Rule: the server authorizes each client NOTIFY with the method authorizer; a denied NOTIFY is dropped and logged.
Rejected: connect-time authorization only, because 14 already states that it is not enough.

### Credential expiry grace and detached sessions
7.5 allows a grace period without a default and covers neither a refresh racing the expiry nor a detached session whose credential expires.
Rule: the default grace is 0 s. An AUTH_REFRESH received before the close is processed first. Detached sessions run no expiry timer; a resume with an expired credential and no fresh `auth` gets RESUME_FAILED.
Rejected: a 5 s grace with expiry ending detached sessions, because it needs a timer per detached session and loses sessions whose clients resume with fresh credentials.

## Channels and presence

### Offsets on channels without history
10.5 gives every channel gapless offsets but requires a single sequencer only for channels with history.
Rule: every channel has a single sequencer. In multi-node deployments, publications on channels sequenced elsewhere fail during a backplane outage.
Rejected: node-local epochs, because clients would see constant epoch changes; advisory offsets, because snapshot positions and client gap checks would lose meaning.

### Typed subscription parameters
10.2 types the SUBSCRIBE data section per the channel declaration, but 12.2 has no field for it.
Rule: channels gain an optional `subscribe` type field; the data section is one value of that type.
Rejected: a `params` list like methods, because adding a parameter would then be a breaking change (12.7).

### Snapshot position and replay on subscribe
10.2 defines SUBSCRIBED `offset` as the top offset, yet a snapshot provider may return older state, and it is unclear whether `since` replay still runs alongside a snapshot.
Rule: with a snapshot, `offset` is the snapshot's offset, no `since` replay is sent, and every publication above `offset` follows without gaps. A snapshot older than retained history gets REQUEST_ERROR UNAVAILABLE.
Rejected: requiring providers to return state exactly at the top, because most application databases cannot do that without a lock.

### Channel names without a colon
10.1 does not say whether the empty name, a bare name or an empty namespace or instance is valid.
Rule: names are 1 to 255 bytes; a name without `:` is its own namespace; an empty namespace or an empty part after `:` is invalid.
Rejected: requiring a `:` and an instance, because singleton channels would need a dummy instance.

### Presence over a cap or on a namespace without presence
10.6 has no behavior for a channel over the presence cap, for a namespace with presence disabled, or for deltas about unseen members.
Rule: SUBSCRIBE with `presence: true` over the cap gets REQUEST_ERROR RESOURCE_EXHAUSTED; on a namespace without presence, FAILED_PRECONDITION. Clients ignore a leave for an unknown member and treat a join for a present member as an update.
Rejected: subscribing without presence and flagging it, or sending a truncated snapshot, because both need new header keys and the second needs partial-list logic in clients.

## Multi-node

### What fails during a backplane outage
13.8 omitted remote group operations, keyed calls and SUBSCRIBE with `since`, and did not say whether a failed broad send reaches local members.
Rule: a send whose targets may include remote sessions fails with UNAVAILABLE and reaches no session. Remote group operations and SUBSCRIBE with `since` fail with UNAVAILABLE; keyed calls fail with UNAVAILABLE and `nx: true`. A group change is visible to every send issued after it returns.
Rejected: delivering locally and reporting failure, because notifications are not idempotent and a retry would duplicate local deliveries.

### SUBSCRIBE without since during an outage
13.8 failed SUBSCRIBE with `since` during a backplane outage but said nothing about a plain SUBSCRIBE to a channel sequenced on another node, whose SUBSCRIBED needs the current offset.
Rule: it fails with REQUEST_ERROR UNAVAILABLE, like SUBSCRIBE with `since`.
Rejected: subscribing with a pending position, because SUBSCRIBED would carry no valid `offset` and clients would need a second message to learn it.

### Presence after a partition longer than the lease
An outage longer than the presence lease makes other nodes emit leaves for members that are still connected, followed by joins on re-announce.
Rule: accept and document the flap; a partitioned node is indistinguishable from a dead one.
Rejected: longer leases, because every real node death would be reported later.

## Versioning

### Conformance levels versus bindings and codecs
17.1 put the credential lifecycle in Core, which the MVP cannot pass, and defined a WebTransport level that mixes a binding with a feature.
Rule: levels describe features and the matrix describes where they run. Core covers framing, 7.1-7.4, initial authentication from 7.5, 7.6, 7.7, 7.10, unary calls and notifications. New levels: Auth lifecycle (the rest of 7.5, plus 7.9) and Lanes (9.2). The WebTransport level is dropped.
Rejected: pulling the credential lifecycle into the MVP, because it grows M2 to M7 with M11 work; a partial Core claim, because the MVP would advertise a level it does not pass.

### Protocol version numbers after 1.0
HELLO `versions` carries integers, and the spec did not say whether a 1.x revision changes them.
Rule: `versions` carries the major version only; compatible revisions keep `1` and negotiate their additions as capabilities (7.10).
Rejected: a revision field, because it invites version sniffing instead of capability checks.

## Follow-up: draft 0.9

### Data-section encoding order
Headers had a deterministic encoding (5.4.1) but data sections had none, so struct and map values had several valid byte forms and codec vectors could not be byte-exact.
Rule: one encoding per value. Struct fields in manifest declaration order, `map` keys sorted, shortest integer and float forms, compact `json` with RFC 8785 strings and numbers. Decoders accept any order.
Rejected: full RFC 8949 deterministic order for structs too (every struct encode sorts its fields, and the order differs from the manifest); no rule with value comparison in vectors (byte-level interop tests and hashing of payloads become unreliable).
