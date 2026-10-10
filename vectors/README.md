# Test vectors

Language-neutral test fixtures for the Pavia protocol ([spec 17.2](../spec/pavia-protocol.md#172-test-vectors)). The Rust and TypeScript suites, the conformance runners and the `pavia` CLI all read these files. Nothing here is sent on the wire.

```
vectors/
  README.md            this file: the formats, field by field
  schema/              JSON Schema (2020-12) for each kind, plus invalid/ fixtures CI must reject
  frames/<group>.json  frame vectors
  codecs/<group>.json  codec vectors
  varints/<group>.json varint vectors
  scripts/<group>.json scripts and abuse scripts
```

## Common rules

- One JSON file per group. Each file has `group` (its file name without `.json`), an optional `note`, and `cases`. The `$schema` key points at the schema the file follows.
- Every case has `id` (kebab-case, unique within its kind), `spec` (the sections it covers, for example `["5.2", "7.1"]` or `["Appendix A"]`) and `note` (one or two sentences saying what the case shows).
- Hex is lowercase. Spaces between bytes are allowed and ignored; use them to separate fields so a reviewer can check a vector against [spec 5.2](../spec/pavia-protocol.md#52-frame-layout) by eye.
- Headers and data are strings in CBOR diagnostic notation ([RFC 8949 section 8](https://www.rfc-editor.org/rfc/rfc8949#section-8)), the notation the spec uses: integer map keys, `h'...'` for byte strings, `37(h'...')` for tags. Only the basic notation is used: no comments, no encoding indicators, no concatenated strings.
- `direction: both` means a decoder MUST turn the hex into the decoded form and an encoder MUST turn the decoded form into exactly the hex. `decode` means only the first: the hex is something a conforming encoder never produces (keys out of deterministic order, an invalid input), or the encoding is not unique.

## Frame vectors (`frames/`)

| Field | Required | Meaning |
|---|---|---|
| `binding` | yes | `ws` or `wt` |
| `direction` | yes | `both` or `decode` |
| `hex` | yes | The input bytes. For `ws`: one WebSocket binary message, a sequence of `WsFrame`s ([spec 5.4](../spec/pavia-protocol.md#54-frames-on-websocket)). For `wt`: bytes read from one stream, frames back to back ([spec 5.3](../spec/pavia-protocol.md#53-frames-on-webtransport-streams)) |
| `fin` | `wt` only | `true` if the stream ends (FIN) after these bytes. Absent means `false` |
| `frames` | one of | The frames the decoder yields, in order. An empty list means "valid so far, needs more bytes" |
| `error` | one of | `{code, reason?}`: the close code from [spec 15.1](../spec/pavia-protocol.md#151-session-close-codes) a receiver uses. Error cases are always `decode` |

Each element of `frames`:

| Field | Meaning |
|---|---|
| `stream` | Stream ID; present for `ws`, absent for `wt` (the WebTransport stream is outside the bytes) |
| `type` | Frame type as an integer ([spec 5.6](../spec/pavia-protocol.md#56-frame-type-registry)) |
| `name` | Frame name, for readers only |
| `flags` | Flags byte as an integer |
| `header` | Diagnostic notation, or `null` for an empty header (HeaderLength 0, [spec 5.2](../spec/pavia-protocol.md#52-frame-layout)) |
| `data` | Diagnostic notation for a `cbor` data section, the exact JSON text for a `json` one, or `null` when empty |
| `codec` | `cbor` or `json`; required when `data` is not `null` |
| `data_hex` | Raw data bytes for frames whose data is opaque rather than codec-encoded (PING, PONG); `data` is then `null` |

A `both` vector's header is in deterministic encoding ([spec 5.4.1](../spec/pavia-protocol.md#541-header-section-encoding)), so the header string must decode to exactly those bytes.

## Codec vectors (`codecs/`)

A file may define named types in `types`, in the form of the manifest's `types` object ([spec 12.2](../spec/pavia-protocol.md#122-manifest-format)). Each case:

| Field | Required | Meaning |
|---|---|---|
| `type` | yes | A contract type in manifest syntax: `"u64"`, `{"list": "string"}`, `{"ref": "Message"}` |
| `direction` | yes | `both` or `decode` |
| `value` | one of | The value in diagnostic notation |
| `cbor` | with `value` | The `cbor` encoding, as hex |
| `json` | with `value`, or with `error` | The `json` encoding as text, compared byte for byte: compact, no insignificant whitespace |
| `error` | one of | `{codec, reason?}`: decoding the given `cbor` or `json` input MUST fail. Error cases are always `decode` |

Every value has one encoding ([spec 12.3](../spec/pavia-protocol.md#123-codec-mapping), draft 0.9): struct fields in manifest declaration order, `map` keys sorted, shortest integer and float forms, compact `json`. Codec vectors are therefore `both` unless the input is one that encoders never produce, such as fields out of order.

## Varint vectors (`varints/`)

QUIC variable-length integers ([spec 5.1](../spec/pavia-protocol.md#51-variable-length-integers)), the prefix encoding inside frames and the WebSocket envelope. Values are decimal strings, because the largest ones do not fit a JSON number that every parser reads exactly. Each case is one of:

| Fields | `direction` | Meaning |
|---|---|---|
| `hex`, `value` | `both` or `decode` | `hex` decodes to `value` and uses every byte; for `both`, encoding `value` gives exactly `hex`, the shortest form. `decode` marks a longer form that decoders accept and encoders never write |
| `hex`, `need_more: true` | `decode` | `hex` is the start of a varint whose prefix announces more bytes than are present: the decoder asks for more input and consumes nothing |
| `value`, `error: "out_of_range"` | `encode` | `value` is above 2^62-1, so it has no encoding and the encoder MUST refuse it |

## Scripts (`scripts/`)

A script is an exchange seen from the client's side ([spec 17.2](../spec/pavia-protocol.md#172-test-vectors)). The server conformance runner plays the script against a server under test; the client conformance runner plays the server's side against a client under test.

| Field | Required | Meaning |
|---|---|---|
| `binding` | yes | `ws`, `wt`, or `any` (the scenario is the same on both) |
| `codec` | yes | The codec the client offers in HELLO |
| `requires` | no | Capabilities the server under test must be configured to grant, for example `["resume"]` |
| `steps` | yes | Ordered list; each step is an object with exactly one key, the verb |

Verbs:

| Verb | Value | Meaning |
|---|---|---|
| `send` | a frame, plus optional `repeat` | Send the frame (`repeat` times, default 1). A `send` after `close_transport` opens a new transport to the same endpoint |
| `expect` | a frame pattern, plus optional `capture` | The next frame received on that stream MUST match |
| `expect_nothing_within` | `{ms}` | No frame arrives for `ms` milliseconds |
| `wait` | `{ms}` | Advance time by `ms`. Runners on an in-memory driver advance the clock; runners on a real socket sleep |
| `close_transport` | `{}` | Drop the transport without CLOSE |
| `expect_close` | `{code, within_ms}` | The transport closes with that code ([spec 15.1](../spec/pavia-protocol.md#151-session-close-codes); on WebSocket `4000 + code`) within `within_ms`. Frames received before the close are ignored |

Frames in scripts use the same fields as frame vectors, with `stream` always present. `flags` defaults to 0.

**Patterns.** In an `expect` header or data string:

- `*` matches any single value.
- A map written with `...` as its last entry matches a map that has at least the listed keys with matching values; without `...` the key sets must be equal. `{15: 1, 17: "cbor", ...}` matches any WELCOME with version 1 and codec `cbor`.
- `$name` matches only a value equal to a capture made earlier.

**Captures.** `"capture": {"token": "header.13"}` stores the value at that path of the matched frame under the name `token`. Paths are `header` or `data` followed by `.key` segments (map keys or array indexes). A later `send` or `expect` writes `$token` where the value goes; the runner substitutes the stored CBOR value. Referring to a name that was never captured is an error in the script, reported before it runs.

**Heartbeats.** The runner answers every PING from the peer with PONG on its own ([spec 7.4](../spec/pavia-protocol.md#74-heartbeat-ping-0x04-and-pong-0x05)) and does not pass PING or PONG frames to `expect`, unless the step expects that frame type.

**Abuse scripts** (`scripts/abuse.json`) drive a limit from [spec 16](../spec/pavia-protocol.md#16-defaults) past its bound and end with `expect_close`.

## Validating

CI validates every file against its schema and checks that the fixtures in `schema/invalid/` are rejected. To run the same check locally:

```
uvx check-jsonschema --schemafile vectors/schema/frames.schema.json vectors/frames/*.json
uvx check-jsonschema --schemafile vectors/schema/codecs.schema.json vectors/codecs/*.json
uvx check-jsonschema --schemafile vectors/schema/varints.schema.json vectors/varints/*.json
uvx check-jsonschema --schemafile vectors/schema/scripts.schema.json vectors/scripts/*.json
```

Schema validation checks structure only. The content is checked twice, by independent implementations in each language: `crates/pavia-vectors` (Rust) and `clients/typescript/packages/vectors` (TypeScript) each parse the diagnostic notation, lay the decoded form out as a frame and compare it with `hex`, encode or decode every codec value, encode or decode every varint, and check that scripts parse and capture every `$name` before using it. Both run with the normal test commands:

```
cargo test -p pavia-vectors
cd clients/typescript && pnpm test
```

## Adding vectors

- A behavior change updates the spec and the vectors in the same pull request ([spec README](../spec/README.md)).
- Work bytes out by hand against the spec, or print them with `pavia decode` once the CLI exists (#6), and review them field by field.
- Prefer a new case in an existing group over a new file; add a file when a new area of the spec starts.
