import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { type Diag, hexToBytes, parse } from "@pavia/vectors/diag";
import { describe, expect, it } from "vitest";
import {
  type Codec,
  type ContractType,
  DataError,
  decodeData,
  encodeData,
  type Field,
  type Types,
} from "../src/index.js";
import { encodeError, value as itemOf } from "./support.js";

const VECTORS = fileURLToPath(new URL("../../../../../vectors/codecs/", import.meta.url));

interface CodecCase {
  readonly id: string;
  readonly type: ContractType;
  readonly direction: "both" | "decode";
  readonly value?: string;
  readonly cbor?: string;
  readonly json?: string;
  readonly error?: { readonly codec: Codec; readonly reason?: string };
}

const FILES = readdirSync(VECTORS)
  .filter((f) => f.endsWith(".json"))
  .sort()
  .map((file) => JSON.parse(readFileSync(VECTORS + file, "utf8")) as { group: string; types?: Types; cases: CodecCase[] });

const TEXT = new TextEncoder();

function hex(bytes: Uint8Array): string {
  return [...bytes].map((b) => b.toString(16).padStart(2, "0")).join(" ");
}

function canonicalHex(text: string): string {
  return hex(hexToBytes(text));
}

function typed(type: ContractType, types: Types, d: Diag): unknown {
  if (typeof type === "string") {
    switch (type) {
      case "unit":
        return null;
      case "any":
        return itemOf(d);
      case "i64":
      case "u64":
        return d.kind === "uint" ? (d as { value: bigint }).value : -1n - (d as { value: bigint }).value;
      case "timestamp": {
        const content = (d as { value: Diag }).value;
        if (content.kind === "float") return new Date(Math.round(content.value * 1000));
        const seconds = content.kind === "uint" ? (content as { value: bigint }).value : -1n - (content as { value: bigint }).value;
        return new Date(Number(seconds * 1000n));
      }
      case "uuid": {
        const h = hex((d as { value: { value: Uint8Array } }).value.value).replaceAll(" ", "");
        return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
      }
      default:
        if (d.kind === "uint") return Number(d.value);
        if (d.kind === "nint") return Number(-1n - d.value);
        return (d as { value: unknown }).value;
    }
  }
  if ("option" in type) return d.kind === "null" ? null : typed(type.option, types, d);
  if ("list" in type) return (d as { items: Diag[] }).items.map((item) => typed(type.list, types, item));
  if ("tuple" in type) return (d as { items: Diag[] }).items.map((item, i) => typed(type.tuple[i] as ContractType, types, item));
  if ("struct" in type) return structOf(type.struct.fields, types, d);
  if ("map" in type) {
    return new Map((d as { entries: [Diag, Diag][] }).entries.map(([k, v]) => [(k as { value: string }).value, typed(type.map, types, v)]));
  }
  const named = types[type.ref];
  if (named === undefined) throw new Error(`no type ${type.ref}`);
  if ("alias" in named) return typed(named.alias, types, d);
  if ("struct" in named) return structOf(named.struct.fields, types, d);
  if (d.kind === "text") return { kind: d.value };
  const [name, payload] = (d as { entries: [Diag, Diag][] }).entries[0] as [Diag, Diag];
  const variant = named.enum.variants.find((v) => v.name === (name as { value: string }).value);
  return { kind: (name as { value: string }).value, value: typed(variant?.type as ContractType, types, payload) };
}

function structOf(fields: readonly Field[], types: Types, d: Diag): Record<string, unknown> {
  const entries = (d as { entries: [Diag, Diag][] }).entries;
  const out: Record<string, unknown> = {};
  for (const field of fields) {
    const entry = entries.find(([k]) => (k as { value: string }).value === field.name);
    out[field.name] = entry === undefined ? null : typed(field.type, types, entry[1]);
  }
  return out;
}

describe("codec vectors", () => {
  for (const file of FILES) {
    const types = file.types ?? {};
    for (const c of file.cases) {
      it(`${file.group}#${c.id}`, () => {
        if (c.error !== undefined) {
          const input = c.error.codec === "cbor" ? hexToBytes(c.cbor as string) : TEXT.encode(c.json as string);
          let error: unknown;
          try {
            decodeData(c.type, types, input, c.error.codec);
          } catch (e) {
            error = e;
          }
          expect(error).toBeInstanceOf(DataError);
          expect((error as DataError).codec).toBe(c.error.codec);
          return;
        }
        const expected = typed(c.type, types, parse(c.value as string));
        if (c.cbor !== undefined) {
          expect(decodeData(c.type, types, hexToBytes(c.cbor), "cbor")).toEqual(expected);
          if (c.direction === "both") expect(hex(encodeData(c.type, types, expected, "cbor"))).toBe(canonicalHex(c.cbor));
        }
        if (c.json !== undefined) {
          expect(decodeData(c.type, types, TEXT.encode(c.json), "json")).toEqual(expected);
          if (c.direction === "both") {
            expect(new TextDecoder().decode(encodeData(c.type, types, expected, "json"))).toBe(c.json);
          }
        }
      });
    }
  }

  it("covers every case", () => {
    expect(FILES.reduce((n, f) => n + f.cases.length, 0)).toBe(101);
  });
});

const TYPES: Types = {
  Status: {
    enum: { variants: [{ name: "Idle" }, { name: "Busy", type: "u32" }], open: true },
  },
  Closed: { enum: { variants: [{ name: "A" }] } },
  Point: { struct: { fields: [{ name: "x", type: "f64" }, { name: "y", type: "f64" }] } },
  Options: { struct: { fields: [{ name: "lang", type: { option: "string" }, optional: true }] } },
  Loop: { alias: { ref: "Loop" } },
};

function both(type: ContractType, value: unknown): void {
  for (const codec of ["cbor", "json"] as const) {
    expect(decodeData(type, TYPES, encodeData(type, TYPES, value, codec), codec)).toEqual(value);
  }
}

function dataError(f: () => unknown): DataError {
  let error: unknown;
  try {
    f();
  } catch (e) {
    error = e;
  }
  expect(error).toBeInstanceOf(DataError);
  return error as DataError;
}

const json = (text: string): Uint8Array => TEXT.encode(text);

describe("typed codec", () => {
  it("maps variants an open enum does not declare to the catch-all, and refuses to encode it", () => {
    expect(decodeData({ ref: "Status" }, TYPES, json('"Gone"'), "json")).toEqual({ kind: "Unknown", name: "Gone" });
    expect(decodeData({ ref: "Status" }, TYPES, json('{"Moved":[1]}'), "json")).toEqual({ kind: "Unknown", name: "Moved" });
    expect(decodeData({ ref: "Status" }, TYPES, hexToBytes("64 47 6f 6e 65"), "cbor")).toEqual({ kind: "Unknown", name: "Gone" });
    expect(encodeError(() => encodeData({ ref: "Status" }, TYPES, { kind: "Unknown", name: "Gone" }, "cbor")).kind).toBe(
      "unknown_variant",
    );
    expect(dataError(() => decodeData({ ref: "Closed" }, TYPES, json('"B"'), "json")).reason).toBe("unknown_variant");
    expect(dataError(() => decodeData({ ref: "Status" }, TYPES, json('{"Idle":1}'), "json")).reason).toBe("unknown_variant");
    expect(dataError(() => decodeData({ ref: "Status" }, TYPES, json('"Busy"'), "json")).reason).toBe("unknown_variant");
    both({ ref: "Status" }, { kind: "Busy", value: 7 });
  });

  it("ignores unknown struct fields", () => {
    expect(decodeData({ ref: "Point" }, TYPES, json('{"z":[1,{"a":null}],"x":1,"y":2}'), "json")).toEqual({ x: 1, y: 2 });
    expect(decodeData({ ref: "Point" }, TYPES, hexToBytes("a3 61 7a 80 61 78 f9 3c 00 61 79 f9 40 00"), "cbor")).toEqual({
      x: 1,
      y: 2,
    });
  });

  it("omits optional fields that are null or absent, and writes null for a required option", () => {
    expect(new TextDecoder().decode(encodeData({ ref: "Options" }, TYPES, { lang: null }, "json"))).toBe("{}");
    expect(new TextDecoder().decode(encodeData({ ref: "Options" }, TYPES, {}, "json"))).toBe("{}");
    expect(hex(encodeData({ option: "string" }, TYPES, undefined, "cbor"))).toBe("f6");
    expect(encodeError(() => encodeData({ ref: "Point" }, TYPES, { x: 1 }, "json")).path).toBe("$.y");
  });

  it("names the place of an error in its path", () => {
    const type: ContractType = { list: { map: { ref: "Point" } } };
    const value = [new Map([["a", { x: 1, y: 2 }]]), new Map([["b", { x: 1, y: "2" }]])];
    expect(encodeError(() => encodeData(type, TYPES, value, "cbor")).path).toBe('$[1]["b"].y');
    expect(dataError(() => decodeData(type, TYPES, json('[{},{"b":{"x":1,"y":"2"}}]'), "json")).path).toBe('$[1]["b"].y');
  });

  it("refuses values a receiver would refuse", () => {
    expect(encodeError(() => encodeData("f32", TYPES, 0.1, "cbor")).kind).toBe("out_of_range");
    expect(encodeError(() => encodeData("f64", TYPES, Number.NaN, "json")).kind).toBe("not_json");
    expect(hex(encodeData("f64", TYPES, Number.NaN, "cbor"))).toBe("f9 7e 00");
    expect(encodeError(() => encodeData("u8", TYPES, 256, "cbor")).kind).toBe("out_of_range");
    expect(encodeError(() => encodeData("u8", TYPES, 1.5, "cbor")).kind).toBe("invalid_value");
    expect(encodeError(() => encodeData("i64", TYPES, 1, "json")).kind).toBe("invalid_value");
    expect(encodeError(() => encodeData("uuid", TYPES, "00112233-4455-6677-8899-AABBCCDDEEFF", "cbor")).kind).toBe(
      "invalid_value",
    );
    expect(encodeError(() => encodeData("timestamp", TYPES, new Date(Number.NaN), "cbor")).kind).toBe("out_of_range");
    expect(encodeError(() => encodeData("timestamp", TYPES, new Date(Date.UTC(10_000, 0)), "json")).kind).toBe(
      "out_of_range",
    );
    expect(encodeError(() => encodeData({ ref: "Missing" }, TYPES, 1, "cbor")).kind).toBe("unknown_type");
    expect(dataError(() => decodeData({ ref: "Missing" }, TYPES, hexToBytes("01"), "cbor")).reason).toBe("unknown_type");
    expect(encodeError(() => encodeData("string", TYPES, "\ud800", "json")).kind).toBe("lone_surrogate");
    expect(encodeError(() => encodeData("any", TYPES, { kind: "bytes", value: new Uint8Array(1) }, "json")).kind).toBe(
      "not_json",
    );
  });

  it("refuses to encode a value nested deeper than the limit", () => {
    let type: ContractType = "u8";
    let value: unknown = 1;
    for (let i = 0; i < 17; i++) {
      type = { list: type };
      value = [value];
    }
    expect(encodeError(() => encodeData(type, TYPES, value, "cbor")).kind).toBe("too_deep");
    expect(encodeError(() => encodeData(type, TYPES, value, "json")).kind).toBe("too_deep");
    expect(() => encodeData(type, TYPES, value, "cbor", { maxDepth: 17 })).not.toThrow();
  });

  it("writes timestamps as whole seconds or the narrowest float that keeps the milliseconds", () => {
    expect(hex(encodeData("timestamp", TYPES, new Date(-127_938), "cbor"))).toBe("c1 fa c2 ff e0 42");
    expect(hex(encodeData("timestamp", TYPES, new Date(500), "cbor"))).toBe("c1 f9 38 00");
    expect(hex(encodeData("timestamp", TYPES, new Date(-1000), "cbor"))).toBe("c1 20");
    both("timestamp", new Date(-62_167_219_200_000));
    both("timestamp", new Date(253_402_300_799_999));
    expect(new TextDecoder().decode(encodeData("timestamp", TYPES, new Date(-62_167_219_200_000), "json"))).toBe(
      '"0000-01-01T00:00:00.000Z"',
    );
    expect(dataError(() => decodeData("timestamp", TYPES, json('"2025-02-29T00:00:00.000Z"'), "json")).reason).toBe(
      "not_canonical",
    );
  });

  it("keeps any values as CBOR items in both codecs", () => {
    const item = { kind: "map", entries: [[{ kind: "text", value: "b" }, { kind: "float", value: 0.5 }], [{ kind: "text", value: "a" }, { kind: "nint", value: 0n }]] };
    expect(new TextDecoder().decode(encodeData("any", TYPES, item, "json"))).toBe('{"b":0.5,"a":-1}');
    both("any", item);
    const tagged = { kind: "tag", tag: 5n, item: { kind: "uint", value: 1n } };
    expect(decodeData("any", TYPES, encodeData("any", TYPES, tagged, "cbor"), "cbor")).toEqual(tagged);
    expect(encodeError(() => encodeData("any", TYPES, tagged, "json")).kind).toBe("not_json");
    expect(dataError(() => decodeData("any", TYPES, json("1e400"), "json")).reason).toBe("out_of_range");
    expect(decodeData("any", TYPES, json("18446744073709551616"), "json")).toEqual({ kind: "float", value: 2 ** 64 });
  });

  it("parses JSON strictly", () => {
    for (const text of ["", " ", "NaN", "-Infinity", "[1,]", '{"a":1,}', "01", "1.", ".5", '"\\ud800"', '"\\udc00x"', '"\t"', "\ufeff1", "[1] x", "tru"]) {
      expect(dataError(() => decodeData("any", TYPES, json(text), "json")).reason, text).toBe("malformed");
    }
    expect(dataError(() => decodeData("any", TYPES, Uint8Array.of(0x22, 0xff, 0x22), "json")).reason).toBe("malformed");
    expect(decodeData("string", TYPES, json(' "\\ud83d\\ude00\\/\\u00e9" '), "json")).toBe("😀/é");
    expect(dataError(() => decodeData("any", TYPES, json('{"a":1,"a":1}'), "json")).reason).toBe("duplicate_key");
  });

  it("reads bytes and floats as the codecs allow", () => {
    both("bytes", Uint8Array.of(1, 2, 3, 4, 5));
    both("bytes", Uint8Array.of(0xff));
    expect(dataError(() => decodeData("bytes", TYPES, json('"A"'), "json")).reason).toBe("not_canonical");
    expect(dataError(() => decodeData("bytes", TYPES, json('"A+"'), "json")).reason).toBe("not_canonical");
    expect(dataError(() => decodeData("f32", TYPES, hexToBytes("fb 3f b9 99 99 99 99 99 9a"), "cbor")).reason).toBe(
      "out_of_range",
    );
    expect(dataError(() => decodeData("f64", TYPES, json("1e400"), "json")).reason).toBe("out_of_range");
    expect(decodeData("f64", TYPES, json("-0"), "json")).toBe(-0);
    expect(dataError(() => decodeData("f64", TYPES, hexToBytes("01"), "cbor")).reason).toBe("wrong_type");
  });

  it("copies decoded bytes out of the input", () => {
    const input = hexToBytes("42 01 02");
    const bytes = decodeData("bytes", TYPES, input, "cbor") as Uint8Array;
    input[1] = 9;
    expect([...bytes]).toEqual([1, 2]);
  });

  it("refuses an alias that never reaches a type", () => {
    expect(encodeError(() => encodeData({ ref: "Loop" }, TYPES, 1, "cbor")).kind).toBe("unknown_type");
  });
});
