import { ByteWriter, compareBytes } from "./bytes.js";
import { EncodeError, type HeaderErrorReason, ProtocolError } from "./errors.js";

/**
 * One CBOR item of a header section (spec 5.4.1). Integers are `bigint` so every 64-bit value is
 * exact; `nint` holds `n` for the integer `-1 - n`, as CBOR major type 1 does.
 */
export type HeaderValue =
  | { readonly kind: "uint"; readonly value: bigint }
  | { readonly kind: "nint"; readonly value: bigint }
  | { readonly kind: "bytes"; readonly value: Uint8Array }
  | { readonly kind: "text"; readonly value: string }
  | { readonly kind: "array"; readonly items: readonly HeaderValue[] }
  | { readonly kind: "map"; readonly entries: HeaderMap }
  | { readonly kind: "bool"; readonly value: boolean }
  | { readonly kind: "null" }
  | { readonly kind: "undefined" }
  | { readonly kind: "simple"; readonly value: number }
  | { readonly kind: "float"; readonly value: number };

/** The entries of a CBOR map, in the order they were decoded or given. */
export type HeaderMap = readonly (readonly [HeaderValue, HeaderValue])[];

/** The limits a receiver applies to a header section (spec 5.4.1, 16). */
export interface HeaderLimits {
  /** The largest header in bytes, inclusive. Default 16384. */
  readonly maxSize: number;
  /** The deepest nesting of arrays and maps; the header map itself has depth 1. Default 16. */
  readonly maxDepth: number;
}

/** The defaults of spec 5.4.1: 16 KiB and depth 16. */
export const DEFAULT_HEADER_LIMITS: HeaderLimits = { maxSize: 16_384, maxDepth: 16 };

const UNSIGNED = 0;
const NEGATIVE = 1;
const BYTES = 2;
const TEXT = 3;
const ARRAY = 4;
const MAP = 5;
const SIMPLE = 7;
const MAX_U64 = (1n << 64n) - 1n;
const ARGUMENT_WIDTH: Readonly<Record<number, number>> = { 24: 1, 25: 2, 26: 4, 27: 8 };
const WIDTH_INFO: Readonly<Record<number, number>> = { 1: 24, 2: 25, 4: 26, 8: 27 };

/**
 * Encodes `header` as a CBOR map in core deterministic encoding (RFC 8949 section 4.2.1):
 * definite lengths, shortest integer and float forms, and the keys of every map sorted by their
 * encoded bytes. An empty map encodes as `a0`; a frame without a header writes no bytes instead.
 *
 * @throws {@link EncodeError} `duplicate_key` when two keys of one map encode the same,
 * `reserved_simple_value` for a simple value from 24 to 31, and `out_of_range` for an integer
 * outside 0 to 2^64 - 1.
 */
export function encodeHeader(header: HeaderMap): Uint8Array {
  const out = new ByteWriter();
  writeMap(out, header);
  return out.finish();
}

/**
 * Encodes one item deterministically, as {@link encodeHeader} does for map entries.
 *
 * @throws {@link EncodeError} for the same reasons as {@link encodeHeader}.
 */
export function encodeHeaderValue(value: HeaderValue): Uint8Array {
  const out = new ByteWriter();
  writeValue(out, value);
  return out.finish();
}

function writeValue(out: ByteWriter, value: HeaderValue): void {
  switch (value.kind) {
    case "uint":
      return writeHead(out, UNSIGNED, value.value);
    case "nint":
      return writeHead(out, NEGATIVE, value.value);
    case "bytes":
      writeHead(out, BYTES, BigInt(value.value.length));
      return out.bytes(value.value);
    case "text": {
      const utf8 = new TextEncoder().encode(value.value);
      writeHead(out, TEXT, BigInt(utf8.length));
      return out.bytes(utf8);
    }
    case "array":
      writeHead(out, ARRAY, BigInt(value.items.length));
      for (const item of value.items) writeValue(out, item);
      return;
    case "map":
      return writeMap(out, value.entries);
    case "bool":
      return out.byte(value.value ? 0xf5 : 0xf4);
    case "null":
      return out.byte(0xf6);
    case "undefined":
      return out.byte(0xf7);
    case "simple":
      if (!Number.isInteger(value.value) || value.value < 0 || value.value > 255) {
        throw new EncodeError("out_of_range", `simple value ${value.value} is outside 0 to 255`);
      }
      if (value.value >= 24 && value.value < 32) {
        throw new EncodeError("reserved_simple_value", `simple value ${value.value} is reserved`);
      }
      return writeHead(out, SIMPLE, BigInt(value.value));
    case "float":
      return writeFloat(out, value.value);
  }
}

function writeMap(out: ByteWriter, entries: HeaderMap): void {
  const encoded = entries.map(([key, value]) => ({ key: encodeHeaderValue(key), value }));
  encoded.sort((a, b) => compareBytes(a.key, b.key));
  for (let i = 1; i < encoded.length; i++) {
    if (compareBytes((encoded[i - 1] as { key: Uint8Array }).key, (encoded[i] as { key: Uint8Array }).key) === 0) {
      throw new EncodeError("duplicate_key", "header map has a duplicate key");
    }
  }
  writeHead(out, MAP, BigInt(encoded.length));
  for (const { key, value } of encoded) {
    out.bytes(key);
    writeValue(out, value);
  }
}

function writeHead(out: ByteWriter, major: number, argument: bigint): void {
  if (argument < 0n || argument > MAX_U64) {
    throw new EncodeError("out_of_range", `CBOR argument ${argument} is outside 0 to 2^64 - 1`);
  }
  const type = major * 32;
  if (argument < 24n) return out.byte(type + Number(argument));
  const width = argument < 0x100n ? 1 : argument < 0x1_0000n ? 2 : argument < 0x1_0000_0000n ? 4 : 8;
  out.byte(type + (WIDTH_INFO[width] as number));
  for (let i = width - 1; i >= 0; i--) out.byte(Number((argument >> BigInt(8 * i)) & 0xffn));
}

function writeFloat(out: ByteWriter, x: number): void {
  if (Number.isNaN(x)) {
    out.bytes(new Uint8Array([0xf9, 0x7e, 0x00]));
    return;
  }
  const half = halfBits(x);
  const view = new DataView(new ArrayBuffer(9));
  if (half !== undefined) {
    view.setUint8(0, 0xf9);
    view.setUint16(1, half);
    out.bytes(new Uint8Array(view.buffer, 0, 3));
  } else if (Math.fround(x) === x) {
    view.setUint8(0, 0xfa);
    view.setFloat32(1, x);
    out.bytes(new Uint8Array(view.buffer, 0, 5));
  } else {
    view.setUint8(0, 0xfb);
    view.setFloat64(1, x);
    out.bytes(new Uint8Array(view.buffer, 0, 9));
  }
}

function halfBits(x: number): number | undefined {
  if (Math.fround(x) !== x) return undefined;
  const view = new DataView(new ArrayBuffer(4));
  view.setFloat32(0, x);
  const bits = view.getUint32(0);
  const sign = bits >>> 31 ? 0x8000 : 0;
  const exponent = (bits >>> 23) & 0xff;
  const mantissa = bits & 0x7f_ffff;
  if (exponent === 0xff) return sign | 0x7c00;
  if (exponent === 0) return mantissa === 0 ? sign : undefined;
  const e = exponent - 127;
  if (e > 15 || e < -24) return undefined;
  if (e >= -14) return mantissa & 0x1fff ? undefined : sign | ((e + 15) << 10) | (mantissa >>> 13);
  const significand = 0x80_0000 + mantissa;
  const divisor = 2 ** -(e + 1);
  return significand % divisor === 0 ? sign | (significand / divisor) : undefined;
}

/**
 * Decodes a header section as a receiver does (spec 5.4.1). An empty section is no header and
 * returns `undefined`. Unknown keys are kept; the typed frame layer ignores them.
 *
 * @throws {@link ProtocolError} `malformed_header`, with the rule that failed as its `reason`.
 */
export function decodeHeader(bytes: Uint8Array, limits: HeaderLimits = DEFAULT_HEADER_LIMITS): HeaderMap | undefined {
  if (bytes.length === 0) return undefined;
  if (bytes.length > limits.maxSize) throw refuse("too_large");
  const first = bytes[0] as number;
  if (first === 0xbf) throw refuse("indefinite_length");
  if (first >= 0x38 && first <= 0x3b && bytes.length < 2) throw refuse("malformed");
  if (first < 0xa0 || first > 0xbb) throw refuse("not_a_map");
  const reader = new Reader(bytes, limits.maxDepth);
  const map = reader.map(reader.deeper(0));
  if (reader.pos < bytes.length) throw refuse("trailing_bytes");
  return map;
}

function refuse(reason: HeaderErrorReason): ProtocolError {
  return new ProtocolError("malformed_header", { reason });
}

const utf8 = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

class Reader {
  pos = 0;

  constructor(
    private readonly bytes: Uint8Array,
    private readonly maxDepth: number,
  ) {}

  deeper(depth: number): number {
    if (depth + 1 > this.maxDepth) throw refuse("too_deep");
    return depth + 1;
  }

  map(depth: number): HeaderMap {
    const length = this.head();
    const seen = new Set<string>();
    const entries: (readonly [HeaderValue, HeaderValue])[] = [];
    for (let i = 0n; i < length; i++) {
      const start = this.pos;
      const key = this.value(depth);
      const id = identity(key, this.bytes.subarray(start, this.pos));
      if (seen.has(id)) throw refuse("duplicate_key");
      seen.add(id);
      entries.push([key, this.value(depth)]);
    }
    return entries;
  }

  value(depth: number): HeaderValue {
    const first = this.bytes[this.pos];
    if (first === undefined) throw refuse("malformed");
    const major = first >> 5;
    const info = first & 0x1f;
    if (info === 31 && major >= BYTES && major <= MAP) throw refuse("indefinite_length");
    if (major === 6 && info < 28) throw refuse("tag");
    if (info >= 28) throw refuse("malformed");
    switch (major) {
      case UNSIGNED:
        return { kind: "uint", value: this.head() };
      case NEGATIVE:
        return { kind: "nint", value: this.head() };
      case BYTES:
        return { kind: "bytes", value: this.take(this.head()) };
      case TEXT: {
        const raw = this.take(this.head());
        try {
          return { kind: "text", value: utf8.decode(raw) };
        } catch {
          throw refuse("malformed");
        }
      }
      case ARRAY: {
        const inner = this.deeper(depth);
        const length = this.head();
        const items: HeaderValue[] = [];
        for (let i = 0n; i < length; i++) items.push(this.value(inner));
        return { kind: "array", items };
      }
      case MAP:
        return { kind: "map", entries: this.map(this.deeper(depth)) };
      default:
        return this.simple(info);
    }
  }

  private simple(info: number): HeaderValue {
    if (info < 20) {
      this.pos++;
      return { kind: "simple", value: info };
    }
    switch (info) {
      case 20:
      case 21:
        this.pos++;
        return { kind: "bool", value: info === 21 };
      case 22:
        this.pos++;
        return { kind: "null" };
      case 23:
        this.pos++;
        return { kind: "undefined" };
      case 24: {
        const value = this.bytes[this.pos + 1];
        if (value === undefined || value < 32) throw refuse("malformed");
        this.pos += 2;
        return { kind: "simple", value };
      }
      default: {
        const width = ARGUMENT_WIDTH[info] as number;
        const raw = this.take(BigInt(width), 1);
        const view = new DataView(raw.buffer, raw.byteOffset, raw.byteLength);
        const value = width === 2 ? fromHalf(view.getUint16(0)) : width === 4 ? view.getFloat32(0) : view.getFloat64(0);
        return { kind: "float", value };
      }
    }
  }

  private head(): bigint {
    const info = (this.bytes[this.pos] as number) & 0x1f;
    this.pos++;
    if (info < 24) return BigInt(info);
    const width = ARGUMENT_WIDTH[info] as number;
    if (this.bytes.length - this.pos < width) throw refuse("malformed");
    let n = 0n;
    for (let i = 0; i < width; i++) n = n * 256n + BigInt(this.bytes[this.pos + i] as number);
    this.pos += width;
    return n;
  }

  private take(length: bigint, skip = 0): Uint8Array {
    this.pos += skip;
    if (length > BigInt(this.bytes.length - this.pos)) throw refuse("malformed");
    const end = this.pos + Number(length);
    const out = this.bytes.subarray(this.pos, end);
    this.pos = end;
    return out;
  }
}

function identity(key: HeaderValue, raw: Uint8Array): string {
  switch (key.kind) {
    case "uint":
      return `u${key.value}`;
    case "nint":
      return `n${key.value}`;
    case "text":
      return `t${key.value}`;
    case "bytes":
      return `b${hex(key.value)}`;
    default:
      return `r${hex(raw)}`;
  }
}

function hex(bytes: Uint8Array): string {
  let s = "";
  for (const b of bytes) s += b.toString(16).padStart(2, "0");
  return s;
}

function fromHalf(bits: number): number {
  const sign = bits & 0x8000 ? -1 : 1;
  const exponent = (bits >> 10) & 0x1f;
  const fraction = bits & 0x3ff;
  if (exponent === 0) return sign * fraction * 2 ** -24;
  if (exponent === 0x1f) return fraction === 0 ? sign * Infinity : NaN;
  return sign * (1024 + fraction) * 2 ** (exponent - 25);
}
