// A small CBOR codec used only as an oracle for the vectors. Encoding follows spec 12.3: shortest
// integer, length and float forms, map entries in the order given. Decoding accepts any valid
// definite-length encoding.

import type { Diag } from "./diag.js";

class Writer {
  private readonly bytes: number[] = [];

  push(...b: number[]): void {
    this.bytes.push(...b);
  }

  head(major: number, n: bigint): void {
    const m = major << 5;
    if (n < 24n) this.push(m | Number(n));
    else if (n < 0x100n) this.push(m | 24, Number(n));
    else if (n < 0x10000n) this.push(m | 25, ...be(n, 2));
    else if (n < 0x100000000n) this.push(m | 26, ...be(n, 4));
    else this.push(m | 27, ...be(n, 8));
  }

  result(): Uint8Array {
    return Uint8Array.from(this.bytes);
  }
}

function be(n: bigint, width: number): number[] {
  const out: number[] = [];
  for (let i = width - 1; i >= 0; i--) out.push(Number((n >> BigInt(8 * i)) & 0xffn));
  return out;
}

/** Half-precision bits for `x` when the conversion is exact, otherwise undefined. */
function toHalf(x: number): number | undefined {
  if (Number.isNaN(x)) return 0x7e00;
  const sign = x < 0 || Object.is(x, -0) ? 0x8000 : 0;
  const a = Math.abs(x);
  if (a === Infinity) return sign | 0x7c00;
  if (a === 0) return sign;
  const e = Math.floor(Math.log2(a));
  if (e < -24 || e > 15) return undefined;
  if (e < -14) {
    const mant = a / 2 ** -24; // subnormal: multiples of 2^-24
    return Number.isInteger(mant) ? sign | mant : undefined;
  }
  const mant = (a / 2 ** e - 1) * 1024;
  return Number.isInteger(mant) && mant < 1024 ? sign | ((e + 15) << 10) | mant : undefined;
}

function float(w: Writer, x: number): void {
  const half = toHalf(x);
  if (half !== undefined) {
    w.push(0xf9, half >> 8, half & 0xff);
    return;
  }
  const view = new DataView(new ArrayBuffer(8));
  if (Math.fround(x) === x) {
    view.setFloat32(0, x);
    w.push(0xfa, ...new Uint8Array(view.buffer, 0, 4));
  } else {
    view.setFloat64(0, x);
    w.push(0xfb, ...new Uint8Array(view.buffer, 0, 8));
  }
}

/** Encodes a plain value; patterns (`*`, `$name`, open maps) have no encoding. */
export function encode(d: Diag): Uint8Array {
  const w = new Writer();
  write(w, d);
  return w.result();
}

function write(w: Writer, d: Diag): void {
  switch (d.kind) {
    case "uint":
      return w.head(0, d.value);
    case "nint":
      return w.head(1, d.value);
    case "bytes":
      w.head(2, BigInt(d.value.length));
      return w.push(...d.value);
    case "text": {
      const utf8 = new TextEncoder().encode(d.value);
      w.head(3, BigInt(utf8.length));
      return w.push(...utf8);
    }
    case "array":
      w.head(4, BigInt(d.items.length));
      return d.items.forEach((i) => write(w, i));
    case "map":
      if (d.open) throw new Error("open map `...` is a pattern");
      w.head(5, BigInt(d.entries.length));
      return d.entries.forEach(([k, v]) => {
        write(w, k);
        write(w, v);
      });
    case "tag":
      w.head(6, d.tag);
      return write(w, d.value);
    case "bool":
      return w.push(d.value ? 0xf5 : 0xf4);
    case "null":
      return w.push(0xf6);
    case "float":
      return float(w, d.value);
    case "any":
    case "ref":
      throw new Error(`${d.kind} is a pattern`);
  }
}

/** Decodes one CBOR item that must span all of `bytes`. */
export function decode(bytes: Uint8Array): Diag {
  const r = { pos: 0 };
  const item = read(bytes, r);
  if (r.pos !== bytes.length) throw new Error("trailing bytes after CBOR item");
  return item;
}

function read(b: Uint8Array, r: { pos: number }): Diag {
  const first = b[r.pos++];
  if (first === undefined) throw new Error("truncated CBOR");
  const major = first >> 5;
  const info = first & 0x1f;
  const view = new DataView(b.buffer, b.byteOffset, b.byteLength);
  if (major === 7) {
    if (info === 20 || info === 21) return { kind: "bool", value: info === 21 };
    if (info === 22) return { kind: "null" };
    if (info === 25) return { kind: "float", value: fromHalf(view.getUint16((r.pos += 2) - 2)) };
    if (info === 26) return { kind: "float", value: view.getFloat32((r.pos += 4) - 4) };
    if (info === 27) return { kind: "float", value: view.getFloat64((r.pos += 8) - 8) };
    throw new Error(`unsupported simple value ${info}`);
  }
  let n: bigint;
  if (info < 24) n = BigInt(info);
  else if (info <= 27) {
    const width = 1 << (info - 24);
    n = 0n;
    for (let i = 0; i < width; i++) n = (n << 8n) | BigInt(b[r.pos + i] ?? 0);
    r.pos += width;
  } else throw new Error("indefinite or reserved length");
  const len = Number(n);
  switch (major) {
    case 0:
      return { kind: "uint", value: n };
    case 1:
      return { kind: "nint", value: n };
    case 2:
      r.pos += len;
      return { kind: "bytes", value: b.slice(r.pos - len, r.pos) };
    case 3:
      r.pos += len;
      return { kind: "text", value: new TextDecoder("utf-8", { fatal: true }).decode(b.slice(r.pos - len, r.pos)) };
    case 4:
      return { kind: "array", items: Array.from({ length: len }, () => read(b, r)) };
    case 5:
      return {
        kind: "map",
        open: false,
        entries: Array.from({ length: len }, () => [read(b, r), read(b, r)] as [Diag, Diag]),
      };
    default:
      return { kind: "tag", tag: n, value: read(b, r) };
  }
}

function fromHalf(h: number): number {
  const sign = h & 0x8000 ? -1 : 1;
  const exp = (h >> 10) & 0x1f;
  const mant = h & 0x3ff;
  if (exp === 0) return sign * mant * 2 ** -24;
  if (exp === 31) return mant ? NaN : sign * Infinity;
  return sign * (1 + mant / 1024) * 2 ** (exp - 15);
}

/** Value equality that ignores map key order. */
export function same(a: Diag, b: Diag): boolean {
  if (a.kind !== b.kind) return false;
  switch (a.kind) {
    case "uint":
    case "nint":
    case "bool":
    case "text":
      return a.value === (b as typeof a).value;
    case "float":
      return Object.is(a.value, (b as typeof a).value);
    case "bytes": {
      const y = (b as typeof a).value;
      return a.value.length === y.length && a.value.every((v, i) => v === y[i]);
    }
    case "array": {
      const y = (b as typeof a).items;
      return a.items.length === y.length && a.items.every((v, i) => same(v, y[i] as Diag));
    }
    case "map": {
      const y = (b as typeof a).entries;
      return (
        a.entries.length === y.length &&
        a.entries.every(([k, v]) => y.some(([k2, v2]) => same(k, k2) && same(v, v2)))
      );
    }
    case "tag": {
      const y = b as typeof a;
      return a.tag === y.tag && same(a.value, y.value);
    }
    default:
      return true;
  }
}
