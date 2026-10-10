import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { decode, encode, same } from "../src/cbor.js";
import { type Diag, hexToBytes, parse, patternCount, refs } from "../src/diag.js";
import { render } from "../src/json.js";

const VECTORS = fileURLToPath(new URL("../../../../../vectors/", import.meta.url));

type Json = Record<string, unknown>;
type Case = { ctx: string; c: Json; types: Record<string, string | Json> };

function cases(kind: string): Case[] {
  const seen = new Set<string>();
  const out: Case[] = [];
  for (const file of readdirSync(VECTORS + kind).filter((f) => f.endsWith(".json")).sort()) {
    const doc = JSON.parse(readFileSync(VECTORS + kind + "/" + file, "utf8")) as Json;
    expect(doc.group, `${kind}/${file}: group`).toBe(file.replace(/\.json$/, ""));
    for (const c of doc.cases as Json[]) {
      const id = c.id as string;
      expect(seen.has(id), `${kind}: duplicate id ${id}`).toBe(false);
      seen.add(id);
      out.push({ ctx: `${kind}/${file}#${id}`, c, types: (doc.types ?? {}) as Record<string, string | Json> });
    }
  }
  expect(out.length).toBeGreaterThan(0);
  return out;
}

function varint(n: number): number[] {
  if (n < 0x40) return [n];
  if (n < 0x4000) return [0x40 | (n >> 8), n & 0xff];
  if (n < 0x40000000) return [0x80 | (n >>> 24), (n >> 16) & 0xff, (n >> 8) & 0xff, n & 0xff];
  throw new Error("varint too large for a test frame");
}

function readVarint(b: Uint8Array, pos: number): [number, number] {
  const first = b[pos] ?? 0;
  const len = 1 << (first >> 6);
  let v = first & 0x3f;
  for (let i = 1; i < len; i++) v = v * 256 + (b[pos + i] ?? 0);
  return [v, pos + len];
}

/** Bytes of one frame from its decoded form (spec 5.2, and 5.4 for the stream ID). */
function frameBytes(f: Json, binding: string): number[] {
  const header = typeof f.header === "string" ? [...encode(parse(f.header))] : [];
  let data: number[] = [];
  if (typeof f.data === "string") {
    data = f.codec === "json" ? [...new TextEncoder().encode(f.data)] : [...encode(parse(f.data))];
  } else if (typeof f.data_hex === "string") {
    data = [...hexToBytes(f.data_hex)];
  }
  const payload = [...varint(header.length), ...header, ...data];
  const stream = binding === "ws" ? varint(f.stream as number) : [];
  return [...stream, f.type as number, f.flags as number, ...varint(payload.length), ...payload];
}

/** Header bytes of each frame in `b`, for decode-only cases. */
function frameHeaders(b: Uint8Array, binding: string): Uint8Array[] {
  const out: Uint8Array[] = [];
  let pos = 0;
  while (pos < b.length) {
    if (binding === "ws") pos = readVarint(b, pos)[1];
    const [len, start] = readVarint(b, pos + 2);
    const [hlen, hstart] = readVarint(b, start);
    out.push(b.slice(hstart, hstart + hlen));
    pos = start + len;
  }
  return out;
}

describe("frame vectors", () => {
  for (const { ctx, c } of cases("frames")) {
    const frames = c.frames as Json[] | undefined;
    if (!frames) continue; // error cases
    it(ctx, () => {
      const bytes = hexToBytes(c.hex as string);
      const binding = c.binding as string;
      if (c.direction === "both") {
        expect(frames.flatMap((f) => frameBytes(f, binding))).toEqual([...bytes]);
      } else if (frames.length > 0) {
        const headers = frameHeaders(bytes, binding);
        expect(headers.length).toBe(frames.length);
        frames.forEach((f, i) => {
          if (typeof f.header === "string") {
            expect(same(decode(headers[i] as Uint8Array), parse(f.header))).toBe(true);
          }
        });
      }
    });
  }
});

describe("codec vectors", () => {
  for (const { ctx, c, types } of cases("codecs")) {
    it(ctx, () => {
      if (typeof c.json === "string") expect(() => JSON.parse(c.json as string)).not.toThrow();
      if (typeof c.value !== "string" || typeof c.cbor !== "string") return;
      const expected = parse(c.value);
      const bytes = hexToBytes(c.cbor);
      if (c.direction === "both") {
        expect([...encode(expected)]).toEqual([...bytes]);
        if (typeof c.json === "string") {
          expect(render(c.type as string | Json, expected, types), "json is not the canonical form of value").toBe(c.json);
        }
      }
      expect(same(decode(bytes), expected)).toBe(true);
    });
  }
});

function varintBig(n: bigint): number[] {
  const len = n < 0x40n ? 1 : n < 0x4000n ? 2 : n < 0x40000000n ? 4 : 8;
  const out: number[] = [];
  for (let i = len - 1; i >= 0; i--) out.push(Number((n >> BigInt(8 * i)) & 0xffn));
  out[0] = (out[0] ?? 0) | ((31 - Math.clz32(len)) << 6);
  return out;
}

function readVarintBig(b: Uint8Array): [bigint, number] {
  const first = b[0] ?? 0;
  const len = 1 << (first >> 6);
  let v = BigInt(first & 0x3f);
  for (let i = 1; i < len; i++) v = (v << 8n) | BigInt(b[i] ?? 0);
  return [v, len];
}

describe("varint vectors", () => {
  const MAX = (1n << 62n) - 1n;
  for (const { ctx, c } of cases("varints")) {
    it(ctx, () => {
      const value = typeof c.value === "string" ? BigInt(c.value) : undefined;
      const bytes = typeof c.hex === "string" ? hexToBytes(c.hex) : undefined;
      if (bytes && c.need_more === true) {
        expect(bytes.length).toBeLessThan(1 << ((bytes[0] ?? 0) >> 6));
      } else if (bytes && value !== undefined) {
        expect(readVarintBig(bytes)).toEqual([value, bytes.length]);
        if (c.direction === "both") expect(varintBig(value)).toEqual([...bytes]);
        else expect(varintBig(value)).not.toEqual([...bytes]);
      } else if (value !== undefined) {
        expect(value > MAX).toBe(true);
      } else {
        throw new Error(`${ctx}: unknown case shape`);
      }
    });
  }
});

describe("scripts", () => {
  for (const { ctx, c } of cases("scripts")) {
    it(ctx, () => {
      const captured = new Set<string>();
      const steps = c.steps as Json[];
      steps.forEach((step, i) => {
        const [verb, body] = Object.entries(step)[0] as [string, Json];
        if (verb !== "send" && verb !== "expect") return;
        for (const key of ["header", "data"] as const) {
          const text = body[key];
          if (typeof text !== "string") continue;
          const d: Diag = parse(text);
          if (verb === "send") expect(patternCount(d), `${ctx} step ${i}: send uses * or ...`).toBe(refs(d).length);
          for (const name of refs(d)) expect(captured.has(name), `${ctx} step ${i}: $${name} before capture`).toBe(true);
        }
        if (body.capture) Object.keys(body.capture as Json).forEach((k) => captured.add(k));
      });
      if (ctx.includes("abuse")) expect(Object.keys(steps.at(-1) ?? {})).toEqual(["expect_close"]);
    });
  }
});

describe("diagnostic notation parser", () => {
  it("parses patterns", () => {
    const d = parse("{13: *, 19: $session, ...}");
    expect(refs(d)).toEqual(["session"]);
    expect(patternCount(d)).toBe(3);
    expect(() => encode(d)).toThrow();
  });
  it("rejects trailing input", () => {
    expect(() => parse("1 2")).toThrow();
    expect(() => parse("[1,")).toThrow();
  });
  it("round-trips the largest integers", () => {
    expect([...encode(parse("18446744073709551615"))]).toEqual([0x1b, ...Array(8).fill(0xff)]);
    expect([...encode(parse("-18446744073709551616"))]).toEqual([0x3b, ...Array(8).fill(0xff)]);
  });
});
