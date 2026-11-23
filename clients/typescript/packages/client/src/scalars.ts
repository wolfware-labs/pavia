import { toHex } from "./bytes.js";
import { type CborItem, toHalf } from "./cbor.js";

/** The first instant RFC 3339 can write, 0000-01-01T00:00:00.000Z, in milliseconds since the epoch. */
export const MIN_MILLIS = -62_167_219_200_000;
/** The last instant RFC 3339 can write, 9999-12-31T23:59:59.999Z, in milliseconds since the epoch. */
export const MAX_MILLIS = 253_402_300_799_999;

const MILLIS_PER_DAY = 86_400_000;

/**
 * The tag 1 content for `millis` (spec 12.3): whole seconds as an integer, otherwise the narrowest
 * float, half, single or double, whose value lies less than half a millisecond from `millis`. A
 * float exactly half a millisecond away is a tie that decoders could round either way, so it is
 * never chosen.
 */
export function epochSeconds(millis: number): CborItem {
  if (millis % 1000 === 0) return integerItem(BigInt(millis / 1000));
  const seconds = millis / 1000;
  for (const candidate of [toHalf(seconds), Math.fround(seconds)]) {
    if (Math.abs(candidate * 1000 - millis) < 0.5) return { kind: "float", value: candidate };
  }
  return { kind: "float", value: seconds };
}

/**
 * The milliseconds of a tag 1 content, an integer or a float of seconds, within the range RFC 3339
 * can write. A float exactly half a millisecond off rounds away from zero.
 */
export function millisOfEpochSeconds(content: CborItem): number | undefined {
  let millis: number;
  if (content.kind === "float") {
    const exact = content.value * 1000;
    millis = Math.sign(exact) * Math.round(Math.abs(exact)) + 0;
  } else {
    const seconds = integerOfItem(content);
    if (seconds === undefined) return undefined;
    const big = seconds * 1000n;
    if (big < BigInt(MIN_MILLIS) || big > BigInt(MAX_MILLIS)) return undefined;
    millis = Number(big);
  }
  return Number.isFinite(millis) && millis >= MIN_MILLIS && millis <= MAX_MILLIS ? millis : undefined;
}

/** `millis` as RFC 3339 in UTC with milliseconds: `2025-09-22T00:00:00.123Z`. */
export function rfc3339(millis: number): string {
  const days = Math.floor(millis / MILLIS_PER_DAY);
  const ofDay = millis - days * MILLIS_PER_DAY;
  const [year, month, day] = civilFromDays(days);
  const pad = (n: number, width: number): string => String(n).padStart(width, "0");
  const time = `${pad(Math.floor(ofDay / 3_600_000), 2)}:${pad(Math.floor(ofDay / 60_000) % 60, 2)}`;
  return `${pad(year, 4)}-${pad(month, 2)}-${pad(day, 2)}T${time}:${pad(Math.floor(ofDay / 1000) % 60, 2)}.${pad(ofDay % 1000, 3)}Z`;
}

const RFC3339 = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})\.(\d{3})Z$/;

/**
 * The milliseconds of `text` in exactly the form {@link rfc3339} writes, or `undefined` for any
 * other text, an offset other than `Z` or a date that does not exist.
 */
export function millisOfRfc3339(text: string): number | undefined {
  const match = RFC3339.exec(text);
  if (match === null) return undefined;
  const [year, month, day, hour, minute, second, milli] = match.slice(1).map(Number) as [
    number,
    number,
    number,
    number,
    number,
    number,
    number,
  ];
  const valid =
    month >= 1 &&
    month <= 12 &&
    day >= 1 &&
    day <= daysInMonth(year, month) &&
    hour < 24 &&
    minute < 60 &&
    second < 60;
  if (!valid) return undefined;
  return daysFromCivil(year, month, day) * MILLIS_PER_DAY + ((hour * 60 + minute) * 60 + second) * 1000 + milli;
}

function daysInMonth(year: number, month: number): number {
  if (month === 2) return year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0) ? 29 : 28;
  return month === 4 || month === 6 || month === 9 || month === 11 ? 30 : 31;
}

function daysFromCivil(year: number, month: number, day: number): number {
  const y = month <= 2 ? year - 1 : year;
  const era = Math.floor(y / 400);
  const yearOfEra = y - era * 400;
  const dayOfYear = Math.floor((153 * ((month + 9) % 12) + 2) / 5) + day - 1;
  const dayOfEra = yearOfEra * 365 + Math.floor(yearOfEra / 4) - Math.floor(yearOfEra / 100) + dayOfYear;
  return era * 146_097 + dayOfEra - 719_468;
}

function civilFromDays(days: number): [number, number, number] {
  const shifted = days + 719_468;
  const era = Math.floor(shifted / 146_097);
  const dayOfEra = shifted - era * 146_097;
  const yearOfEra = Math.floor(
    (dayOfEra - Math.floor(dayOfEra / 1460) + Math.floor(dayOfEra / 36_524) - Math.floor(dayOfEra / 146_096)) / 365,
  );
  const dayOfYear = dayOfEra - (365 * yearOfEra + Math.floor(yearOfEra / 4) - Math.floor(yearOfEra / 100));
  const shiftedMonth = Math.floor((5 * dayOfYear + 2) / 153);
  const day = dayOfYear - Math.floor((153 * shiftedMonth + 2) / 5) + 1;
  const month = shiftedMonth < 10 ? shiftedMonth + 3 : shiftedMonth - 9;
  return [yearOfEra + era * 400 + (month <= 2 ? 1 : 0), month, day];
}

/** The integer of a `uint` or `nint` item, or `undefined` for any other item. */
export function integerOfItem(item: CborItem): bigint | undefined {
  if (item.kind === "uint") return item.value;
  return item.kind === "nint" ? -1n - item.value : undefined;
}

/** `n` as a `uint` or `nint` item. It must lie in -2^64 to 2^64 - 1, as every contract integer does. */
export function integerItem(n: bigint): CborItem {
  return n < 0n ? { kind: "nint", value: -1n - n } : { kind: "uint", value: n };
}

/**
 * The integer of a decimal string in the one form `json` writes 64-bit integers in:
 * `0|-?[1-9][0-9]*`, with no sign on zero, no leading zeros and nothing around it.
 */
export function integerOfText(text: string): bigint | undefined {
  return /^(0|-?[1-9][0-9]{0,19})$/.test(text) ? BigInt(text) : undefined;
}

/** `bytes` as a lowercase hyphenated UUID: `7d444840-9dc0-11d1-b245-5ffdce74fad2`. */
export function uuidText(bytes: Uint8Array): string {
  const hex = toHex(bytes);
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

/** The 16 bytes of a lowercase hyphenated UUID, or `undefined` for any other text. */
export function uuidOfText(text: string): Uint8Array | undefined {
  if (!UUID.test(text)) return undefined;
  const hex = text.replaceAll("-", "");
  const bytes = new Uint8Array(16);
  for (let i = 0; i < 16; i++) bytes[i] = Number.parseInt(hex.slice(2 * i, 2 * i + 2), 16);
  return bytes;
}

const BASE64URL = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
const BASE64URL_VALUE = new Int8Array(128).fill(-1);
for (let i = 0; i < BASE64URL.length; i++) BASE64URL_VALUE[BASE64URL.charCodeAt(i)] = i;

/** `bytes` as base64url without padding (RFC 4648 section 5). */
export function base64url(bytes: Uint8Array): string {
  let text = "";
  for (let i = 0; i < bytes.length; i += 3) {
    const n = ((bytes[i] as number) << 16) | ((bytes[i + 1] ?? 0) << 8) | (bytes[i + 2] ?? 0);
    const chars = Math.min(4, Math.ceil(((bytes.length - i) * 8) / 6));
    for (let j = 0; j < chars; j++) text += BASE64URL[(n >> (18 - 6 * j)) & 63];
  }
  return text;
}

/**
 * The bytes of unpadded base64url text, or `undefined` for padding, a character outside the
 * alphabet, a length no byte count gives, or nonzero bits after the last byte.
 */
export function bytesOfBase64url(text: string): Uint8Array | undefined {
  if (text.length % 4 === 1) return undefined;
  const out = new Uint8Array(Math.floor((text.length * 6) / 8));
  let bits = 0;
  let acc = 0;
  let at = 0;
  for (let i = 0; i < text.length; i++) {
    const code = text.charCodeAt(i);
    const value = code < 128 ? (BASE64URL_VALUE[code] as number) : -1;
    if (value < 0) return undefined;
    acc = ((acc << 6) | value) & 0xfff;
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      out[at++] = (acc >> bits) & 0xff;
    }
  }
  return (acc & ((1 << bits) - 1)) === 0 ? out : undefined;
}
