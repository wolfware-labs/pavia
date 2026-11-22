import { ByteWriter, compareBytes, isByte } from "./bytes.js";
import { EncodeError, type HeaderErrorReason, ProtocolError } from "./errors.js";

/**
 * One CBOR item of a header section (spec 5.4.1) or a data section (spec 12.3). Integers are
 * `bigint` so every 64-bit value is exact; `nint` holds `n` for the integer `-1 - n`, as CBOR major
 * type 1 does. A `tag` only occurs in data sections; headers refuse tags.
 */
export type CborItem =
  | { readonly kind: "uint"; readonly value: bigint }
  | { readonly kind: "nint"; readonly value: bigint }
  | { readonly kind: "bytes"; readonly value: Uint8Array }
  | { readonly kind: "text"; readonly value: string }
  | { readonly kind: "array"; readonly items: readonly CborItem[] }
  | { readonly kind: "map"; readonly entries: CborMap }
  | { readonly kind: "bool"; readonly value: boolean }
  | { readonly kind: "null" }
  | { readonly kind: "undefined" }
  | { readonly kind: "simple"; readonly value: number }
  | { readonly kind: "float"; readonly value: number }
  | { readonly kind: "tag"; readonly tag: bigint; readonly item: CborItem };

/** The entries of a CBOR map, in the order they were decoded or given. */
export type CborMap = readonly (readonly [CborItem, CborItem])[];

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
const TAG = 6;
const SIMPLE = 7;
/** The largest CBOR integer argument, 2^64 - 1. */
export const MAX_U64 = (1n << 64n) - 1n;
const ARGUMENT_WIDTH: Readonly<Record<number, number>> = { 24: 1, 25: 2, 26: 4, 27: 8 };
const WIDTH_INFO: Readonly<Record<number, number>> = { 1: 24, 2: 25, 4: 26, 8: 27 };

/**
 * Encodes `header` as a CBOR map in core deterministic encoding (RFC 8949 section 4.2.1):
 * definite lengths, shortest integer and float forms, and the keys of every map sorted by their
 * encoded bytes. An empty map encodes as `a0`; a frame without a header writes no bytes instead.
 *
 * @throws {@link EncodeError} `duplicate_key` when two keys of one map encode the same,
 * `reserved_simple_value` for a simple value from 24 to 31, `out_of_range` for an integer
 * outside 0 to 2^64 - 1, and `tag_in_header` for a tag.
 */
export function encodeHeader(header: CborMap): Uint8Array {
  const out = new ByteWriter();
  writeMap(out, header);
  return out.finish();
}

/**
 * Encodes one item deterministically, as {@link encodeHeader} does for map entries.
 *
 * @throws {@link EncodeError} for the same reasons as {@link encodeHeader}.
 */
export function encodeCborItem(value: CborItem): Uint8Array {
  const out = new ByteWriter();
  writeValue(out, value, false);
  return out.finish();
}

/**
 * Appends `value` as a data section item (spec 12.3): tags are written, and map entries keep the
 * order they have, since the typed layer has put them in theirs. Integers, lengths and floats
 * take their shortest form.
 *
 * @throws {@link EncodeError} `reserved_simple_value`, `out_of_range` or `lone_surrogate`, as
 * {@link encodeHeader}.
 */
export function writeCborItem(out: ByteWriter, value: CborItem): void {
  writeValue(out, value, true);
}

const LONE_SURROGATE = /\p{Surrogate}/u;
const UTF8_ENCODER = new TextEncoder();
const SCRATCH = new DataView(new ArrayBuffer(8));

function writeValue(out: ByteWriter, value: CborItem, data: boolean): void {
  switch (value.kind) {
    case "uint":
      return writeHead(out, UNSIGNED, value.value);
    case "nint":
      return writeHead(out, NEGATIVE, value.value);
    case "bytes":
      writeHead(out, BYTES, BigInt(value.value.length));
      return out.bytes(value.value);
    case "text":
      return writeText(out, value.value);
    case "array":
      writeHead(out, ARRAY, BigInt(value.items.length));
      for (const item of value.items) writeValue(out, item, data);
      return;
    case "map":
      if (!data) return writeMap(out, value.entries);
      writeHead(out, MAP, BigInt(value.entries.length));
      for (const [key, item] of value.entries) {
        writeValue(out, key, true);
        writeValue(out, item, true);
      }
      return;
    case "tag":
      if (!data) throw new EncodeError("tag_in_header", `tag ${value.tag} in a header`);
      writeHead(out, TAG, value.tag);
      return writeValue(out, value.item, true);
    case "bool":
      return out.byte(value.value ? 0xf5 : 0xf4);
    case "null":
      return out.byte(0xf6);
    case "undefined":
      return out.byte(0xf7);
    case "simple":
      if (!isByte(value.value)) {
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

/**
 * Appends `text` as a CBOR text string.
 *
 * @throws {@link EncodeError} `lone_surrogate` for text with no UTF-8 form.
 */
export function writeText(out: ByteWriter, text: string): void {
  writeUtf8(out, utf8Of(text));
}

/** Appends already encoded UTF-8 bytes as a CBOR text string. */
export function writeUtf8(out: ByteWriter, utf8: Uint8Array): void {
  writeHead(out, TEXT, BigInt(utf8.length));
  out.bytes(utf8);
}

/**
 * The UTF-8 bytes of `text`.
 *
 * @throws {@link EncodeError} `lone_surrogate` when `text` holds a lone UTF-16 surrogate, which
 * `TextEncoder` would replace with U+FFFD.
 */
export function utf8Of(text: string): Uint8Array {
  if (LONE_SURROGATE.test(text)) {
    throw new EncodeError("lone_surrogate", "text holds a lone UTF-16 surrogate, which has no UTF-8 form");
  }
  return UTF8_ENCODER.encode(text);
}

function writeMap(out: ByteWriter, entries: CborMap): void {
  if (entries.every(([key]) => key.kind === "uint")) return writeUintKeyedMap(out, entries);
  const encoded = entries.map(([key, value]) => ({ key: encodeCborItem(key), value }));
  encoded.sort((a, b) => compareBytes(a.key, b.key));
  for (let i = 1; i < encoded.length; i++) {
    if (compareBytes((encoded[i - 1] as { key: Uint8Array }).key, (encoded[i] as { key: Uint8Array }).key) === 0) {
      throw new EncodeError("duplicate_key", "header map has a duplicate key");
    }
  }
  writeHead(out, MAP, BigInt(encoded.length));
  for (const { key, value } of encoded) {
    out.bytes(key);
    writeValue(out, value, false);
  }
}

function writeUintKeyedMap(out: ByteWriter, entries: CborMap): void {
  const sorted = [...entries].sort(([a], [b]) => {
    const x = (a as { value: bigint }).value;
    const y = (b as { value: bigint }).value;
    return x < y ? -1 : x > y ? 1 : 0;
  });
  writeHead(out, MAP, BigInt(sorted.length));
  let previous: bigint | undefined;
  for (const [key, value] of sorted) {
    const n = (key as { value: bigint }).value;
    if (n === previous) throw new EncodeError("duplicate_key", "header map has a duplicate key");
    previous = n;
    writeHead(out, UNSIGNED, n);
    writeValue(out, value, false);
  }
}

/**
 * Appends a CBOR head in its shortest form.
 *
 * @throws {@link EncodeError} `out_of_range` for an argument outside 0 to 2^64 - 1.
 */
export function writeHead(out: ByteWriter, major: number, argument: bigint): void {
  if (argument < 0n || argument > MAX_U64) {
    throw new EncodeError("out_of_range", `CBOR argument ${argument} is outside 0 to 2^64 - 1`);
  }
  const type = major * 32;
  if (argument < 24n) return out.byte(type + Number(argument));
  const width = argument < 0x100n ? 1 : argument < 0x1_0000n ? 2 : argument < 0x1_0000_0000n ? 4 : 8;
  out.byte(type + (WIDTH_INFO[width] as number));
  for (let i = width - 1; i >= 0; i--) out.byte(Number((argument >> BigInt(8 * i)) & 0xffn));
}

/** Appends `x` as the narrowest CBOR float that holds it exactly, NaN as `f9 7e 00`. */
export function writeFloat(out: ByteWriter, x: number): void {
  const half = Number.isNaN(x) ? 0x7e00 : halfBits(x);
  if (half !== undefined) {
    out.byte(0xf9);
    out.byte(half >>> 8);
    out.byte(half & 0xff);
  } else if (Math.fround(x) === x) {
    out.byte(0xfa);
    SCRATCH.setFloat32(0, x);
    writeScratch(out, 4);
  } else {
    out.byte(0xfb);
    SCRATCH.setFloat64(0, x);
    writeScratch(out, 8);
  }
}

function writeScratch(out: ByteWriter, length: number): void {
  for (let i = 0; i < length; i++) out.byte(SCRATCH.getUint8(i));
}

function halfBits(x: number): number | undefined {
  if (Math.fround(x) !== x) return undefined;
  SCRATCH.setFloat32(0, x);
  const bits = SCRATCH.getUint32(0);
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
export function decodeHeader(bytes: Uint8Array, limits: HeaderLimits = DEFAULT_HEADER_LIMITS): CborMap | undefined {
  if (bytes.length === 0) return undefined;
  if (bytes.length > limits.maxSize) throw refuse("too_large");
  const first = bytes[0] as number;
  if (first === 0xbf) throw refuse("indefinite_length");
  if (first >= 0x38 && first <= 0x3b && bytes.length < 2) throw refuse("malformed");
  if (first < 0xa0 || first > 0xbb) throw refuse("not_a_map");
  const reader = new Reader(bytes, limits.maxDepth, false, refuse);
  const map = reader.map(reader.deeper(0));
  if (reader.pos < bytes.length) throw refuse("trailing_bytes");
  return map;
}

/**
 * Decodes one CBOR item that fills `bytes`, tags allowed and adding no depth, as a data section
 * (spec 12.3) is read before its type is applied. A top-level array or map has depth 1.
 *
 * @throws the error `fail` returns for the first rule that fails, with the same reasons as
 * {@link decodeHeader} but `not_a_map`, `too_large` and `tag`.
 */
export function decodeCborItem(
  bytes: Uint8Array,
  maxDepth: number,
  fail: (reason: HeaderErrorReason) => Error,
): CborItem {
  const reader = new Reader(bytes, maxDepth, true, fail);
  const item = reader.value(0);
  if (reader.pos < bytes.length) throw fail("trailing_bytes");
  return item;
}

function refuse(reason: HeaderErrorReason): ProtocolError {
  return new ProtocolError("malformed_header", { reason });
}

const UTF8_DECODER = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

class Reader {
  pos = 0;

  constructor(
    private readonly bytes: Uint8Array,
    private readonly maxDepth: number,
    private readonly tags: boolean,
    private readonly fail: (reason: HeaderErrorReason) => Error,
  ) {}

  deeper(depth: number): number {
    if (depth + 1 > this.maxDepth) throw this.fail("too_deep");
    return depth + 1;
  }

  map(depth: number): CborMap {
    const length = this.head();
    const seen = new KeySet();
    const entries: (readonly [CborItem, CborItem])[] = [];
    for (let i = 0n; i < length; i++) {
      const start = this.pos;
      const key = this.value(depth);
      if (!seen.add(key, this.bytes, start, this.pos)) throw this.fail("duplicate_key");
      entries.push([key, this.value(depth)]);
    }
    return entries;
  }

  value(depth: number): CborItem {
    const first = this.bytes[this.pos];
    if (first === undefined) throw this.fail("malformed");
    const major = first >> 5;
    const info = first & 0x1f;
    if (info === 31 && major >= BYTES && major <= MAP) throw this.fail("indefinite_length");
    if (info >= 28) throw this.fail("malformed");
    if (major === TAG) {
      if (!this.tags) throw this.fail("tag");
      const tag = this.head();
      return { kind: "tag", tag, item: this.value(depth) };
    }
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
          return { kind: "text", value: UTF8_DECODER.decode(raw) };
        } catch {
          throw this.fail("malformed");
        }
      }
      case ARRAY: {
        const inner = this.deeper(depth);
        const length = this.head();
        const items: CborItem[] = [];
        for (let i = 0n; i < length; i++) items.push(this.value(inner));
        return { kind: "array", items };
      }
      case MAP:
        return { kind: "map", entries: this.map(this.deeper(depth)) };
      default:
        return this.simple(info);
    }
  }

  private simple(info: number): CborItem {
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
        if (value === undefined || value < 32) throw this.fail("malformed");
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
    if (this.bytes.length - this.pos < width) throw this.fail("malformed");
    let n = 0n;
    for (let i = 0; i < width; i++) n = n * 256n + BigInt(this.bytes[this.pos + i] as number);
    this.pos += width;
    return n;
  }

  private take(length: bigint, skip = 0): Uint8Array {
    this.pos += skip;
    if (length > BigInt(this.bytes.length - this.pos)) throw this.fail("malformed");
    const end = this.pos + Number(length);
    const out = this.bytes.subarray(this.pos, end);
    this.pos = end;
    return out;
  }
}

class KeySet {
  private uints: Set<bigint> | undefined;
  private nints: Set<bigint> | undefined;
  private texts: Set<string> | undefined;
  private byteStrings: Set<string> | undefined;
  private raws: Set<string> | undefined;

  add(key: CborItem, bytes: Uint8Array, start: number, end: number): boolean {
    switch (key.kind) {
      case "uint":
        return addNew((this.uints ??= new Set()), key.value);
      case "nint":
        return addNew((this.nints ??= new Set()), key.value);
      case "text":
        return addNew((this.texts ??= new Set()), key.value);
      case "bytes":
        return addNew((this.byteStrings ??= new Set()), hex(key.value));
      default:
        return addNew((this.raws ??= new Set()), hex(bytes.subarray(start, end)));
    }
  }
}

function addNew<T>(set: Set<T>, value: T): boolean {
  if (set.has(value)) return false;
  set.add(value);
  return true;
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
