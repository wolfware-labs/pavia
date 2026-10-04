# pavia-vectors

Test support for the vectors under [`vectors/`](../../vectors/README.md). Not published.

- `diag`: a parser for the CBOR diagnostic notation the vectors use (RFC 8949 section 8, basic notation), plus the script pattern extensions `*`, `...` and `$name`.
- `tests/vectors.rs`: checks every vector file. Frame vectors: the header and data strings, laid out as a frame, give exactly the bytes in `hex`. Codec vectors: `value` encodes to `cbor`, or decodes from it for `decode` cases. Scripts: every pattern parses and every `$name` is captured before use.

The CBOR encoding here uses `ciborium` and serves only as an independent oracle for the fixtures. The protocol crates choose their own CBOR implementation.

```
cargo test -p pavia-vectors
```
