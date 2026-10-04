---
applyTo: "vectors/**"
---

Reviewing test vectors:

- Each file validates against its schema in `vectors/schema/`; `id`, `spec` and `note` are present, ids are unique within their kind, and `spec` cites the sections the case really exercises.
- Check hex against the decoded form field by field: Type, Flags, Length varint, HeaderLength varint, header, data (spec 5.2). A WebSocket case starts with the stream ID varint; a WebTransport case does not (5.3, 5.4). Length counts the payload only.
- A `both` case must be what a conforming encoder produces: deterministic header CBOR (5.4.1) and one encoding per data-section value (12.3). Anything an encoder never emits (keys out of order, non-shortest forms, invalid input) is `decode`.
- Error cases name the close code of section 15.1 that the spec requires for that input.
- Codec `json` text is compact and follows RFC 8785 for strings and numbers; struct members appear in declaration order.
- Scripts use only the verbs, patterns (`*`, `...`, `$name`) and captures defined in `vectors/README.md`; every `$name` is captured before use; abuse scripts end with `expect_close`.
- A new MUST in the spec has a matching case, or the PR says why not.
