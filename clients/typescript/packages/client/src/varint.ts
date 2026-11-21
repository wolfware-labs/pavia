import type { ByteWriter } from "./bytes.js";
import { EncodeError } from "./errors.js";

/** The largest value a varint holds, 2^62 - 1 (spec 5.1). */
export const MAX_VARINT = (1n << 62n) - 1n;

/** What {@link decodeVarint} found at the given offset. */
export type VarintResult =
  | {
      /** A complete varint. */
      readonly kind: "value";
      /** The decoded value. */
      readonly value: bigint;
      /** How many bytes it took: 1, 2, 4 or 8. */
      readonly length: number;
    }
  | {
      /** The input ends before the varint does; nothing was consumed. */
      readonly kind: "need_more";
    }
  | {
      /** A complete varint whose value is above the caller's limit. */
      readonly kind: "above_max";
      /** The decoded value. */
      readonly value: bigint;
      /** The limit that was passed in. */
      readonly max: bigint;
    };

const NEED_MORE: VarintResult = { kind: "need_more" };
const LENGTHS = [1, 2, 4, 8] as const;
const TWO_POW_32 = 0x1_0000_0000n;
const PREFIX: Readonly<Record<number, number>> = { 1: 0x00, 2: 0x40, 4: 0x80, 8: 0xc0 };

/**
 * Reads one QUIC varint (spec 5.1) starting at `offset`. Longer forms than needed are accepted.
 *
 * The value is checked against `max` before the caller reads past the varint, so a length above
 * the field's limit is refused before anything is allocated for it.
 */
export function decodeVarint(bytes: Uint8Array, offset = 0, max: bigint = MAX_VARINT): VarintResult {
  const first = bytes[offset];
  if (first === undefined) return NEED_MORE;
  const length = LENGTHS[first >> 6] as number;
  if (bytes.length - offset < length) return NEED_MORE;
  let value: bigint;
  if (length <= 4) {
    let n = first & 0x3f;
    for (let i = 1; i < length; i++) n = n * 256 + (bytes[offset + i] as number);
    value = BigInt(n);
  } else {
    let high = first & 0x3f;
    for (let i = 1; i < 4; i++) high = high * 256 + (bytes[offset + i] as number);
    let low = 0;
    for (let i = 4; i < 8; i++) low = low * 256 + (bytes[offset + i] as number);
    value = BigInt(high) * TWO_POW_32 + BigInt(low);
  }
  if (value > max) return { kind: "above_max", value, max };
  return { kind: "value", value, length };
}

/**
 * The number of bytes the shortest encoding of `value` takes.
 *
 * @throws {@link EncodeError} `out_of_range` for a value above 2^62 - 1 or not a non-negative integer.
 */
export function varintLength(value: bigint | number): 1 | 2 | 4 | 8 {
  const n = checked(value);
  if (n <= 0x3fn) return 1;
  if (n <= 0x3fffn) return 2;
  if (n <= 0x3fff_ffffn) return 4;
  return 8;
}

/**
 * Appends `value` in its shortest encoding.
 *
 * @throws {@link EncodeError} `out_of_range` for a value above 2^62 - 1; nothing is written then.
 */
export function writeVarint(out: ByteWriter, value: bigint | number): void {
  out.bytes(encodeVarint(value));
}

/**
 * Encodes `value` in its shortest form.
 *
 * @throws {@link EncodeError} `out_of_range` for a value above 2^62 - 1.
 */
export function encodeVarint(value: bigint | number): Uint8Array {
  const n = checked(value);
  const out = new Uint8Array(varintLength(n));
  let rest = n;
  for (let i = out.length - 1; i >= 0; i--) {
    out[i] = Number(rest % 256n);
    rest /= 256n;
  }
  out[0] = (out[0] as number) + (PREFIX[out.length] as number);
  return out;
}

function checked(value: bigint | number): bigint {
  if (typeof value === "number" && !Number.isSafeInteger(value)) {
    throw new EncodeError("out_of_range", `varint value ${value} is not a safe integer`);
  }
  const n = BigInt(value);
  if (n < 0n || n > MAX_VARINT) throw new EncodeError("out_of_range", `varint value ${n} is outside 0 to 2^62 - 1`);
  return n;
}
