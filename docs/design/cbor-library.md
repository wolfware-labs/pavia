# CBOR library for headers and data sections

Decision record for roadmap C1.4 (#12). It also applies to #18 (data sections) and #22 (tags and data-section depth). Status: accepted, 2026-10-10.

## Decision

`pavia-proto` reads and writes CBOR with `minicbor`, through its `Decoder` and `Encoder`. Pavia's own rules from spec 5.4.1 sit in a validating walk that `pavia-proto` owns, on top of the decoder.

```
pavia-proto   validating walk   exactly one map, duplicate keys, indefinite lengths,
                                tags (#22), size, depth; unknown keys ignored
              map loop          known key: typed value
minicbor      Decoder           datatype() looks at the next item without consuming it;
                                typed reads (u16, str, bytes) check range and UTF-8;
                                borrows from the input slice
              Encoder           shortest integer forms, definite lengths
```

## Why

- The receiver rules need item-by-item control. Spec 5.4.1 rejects indefinite-length items and tags anywhere in the header, and limits depth, including inside values under unknown keys. A pull decoder lets the walk check each item before it reads the next one. `Decoder::datatype()` reports `ArrayIndef` and `MapIndef` before any content is read.
- `minicbor` decodes the item headers of all eight major types, the 1, 2, 4 and 8-byte arguments, range checks for narrow integers and UTF-8 checks for text. The walk does not reimplement any of it.
- Borrowed output fits sans-IO. The decoder takes `&'b [u8]` and returns `&'b str` and `&'b [u8]` that point into the input. A header that is already in memory decodes without copying.
- The encoder gives two of the three deterministic rules. `Encoder` writes integers and lengths in their shortest form and writes definite lengths through `array(len)` and `map(len)`. The third rule, key order, is the caller's: sort by encoded bytes (RFC 8949 4.2.1) before encoding.
- The fixtures keep an independent oracle. `pavia-vectors` checks `vectors/` with `ciborium`. With `minicbor` in the protocol crate, a bug in one implementation does not hide the same bug in the other.

## What the walk does not delegate

`Decoder::skip()` is not used for unknown keys. It steps over a whole value but accepts indefinite-length items, tags and any depth inside it; with the `alloc` feature its internal stack grows with the nesting. So `a1 19 04 00 81 9f ff` (key 1024 holding an array that contains an indefinite-length array) passes `skip()` but is a PROTOCOL_ERROR under 5.4.1. The walk visits every item, under known and unknown keys alike, and the same bad bytes fail the same way under both.

Depth follows spec 5.4.1: arrays and maps add a level, the outermost item has depth 1, tags add none. The limits (16 KiB, depth 16) are parameters of the walk, so #18 and #22 reuse it with their own limits.

## Alternatives

- `ciborium`. Its Serde layer has `from_reader_with_recursion_limit`, but it hides indefinite lengths and maps struct fields to text names, while header keys are integers. Its low-level crate, `ciborium-ll`, exposes `Header::Array(None)` and `Header::Map(None)`, but it reads through a `Read` trait, copies text into a caller buffer and has no typed reads. At that layer it needs the same walk as `minicbor`, with more code under it.
- Hand-rolled. No dependency, but the walk must still step over every major type a peer can send under an unknown key: negative integers, floats of three widths, simple values. That adds the argument decoding and UTF-8 checks `minicbor` already has, in a part of the wire layer that faces untrusted input.
