import { type Diag, hexToBytes, parse } from "@pavia/vectors/diag";
import { describe, expect, it } from "vitest";
import {
  type CborItem,
  type Codec,
  type ContractType,
  decodeData,
  decodeHeader,
  encodeData,
  type Field,
  type Types,
} from "../src/index.js";
import { dataError, encodeError, files, protocolError, value as itemOf } from "./support.js";

interface CodecCase {
  readonly id: string;
  readonly type: ContractType;
  readonly direction: "both" | "decode";
  readonly value?: string;
  readonly cbor?: string;
  readonly json?: string;
  readonly error?: { readonly codec: Codec; readonly reason?: string };
}

const FILES = files("codecs").map(({ doc }) => doc as unknown as { group: string; types?: Types; cases: CodecCase[] });

const TEXT = new TextEncoder();
const textOf = (bytes: Uint8Array): string => new TextDecoder().decode(bytes);

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
          expect(dataError(() => decodeData(c.type, types, input, c.error?.codec as Codec)).codec).toBe(c.error.codec);
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
            expect(textOf(encodeData(c.type, types, expected, "json"))).toBe(c.json);
          }
        }
      });
    }
  }

  it("covers every case", () => {
    expect(FILES.reduce((n, f) => n + f.cases.length, 0)).toBe(113);
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
  Nested: { alias: { option: { ref: "Nested" } } },
  PingA: { alias: { option: { ref: "PingB" } } },
  PingB: { alias: { option: { ref: "PingA" } } },
  Twice: { alias: { option: { option: { ref: "Twice" } } } },
  Event: {
    struct: {
      fields: [
        { name: "at", type: { option: "timestamp" }, optional: true },
        { name: "id", type: { option: "uuid" } },
        { name: "extra", type: { option: "any" }, optional: true },
      ],
    },
  },
  Reserved: {
    struct: {
      fields: [
        { name: "__proto__", type: { ref: "Point" } },
        { name: "constructor", type: { option: "string" }, optional: true },
      ],
    },
  },
};

function both(type: ContractType, value: unknown): void {
  for (const codec of ["cbor", "json"] as const) {
    expect(decodeData(type, TYPES, encodeData(type, TYPES, value, codec), codec)).toEqual(value);
  }
}

const json = (text: string): Uint8Array => TEXT.encode(text);
const UUID = "00112233-4455-6677-8899-aabbccddeeff";

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

  it("refuses tags inside unknown struct fields and names outside the name rule in open enums", () => {
    const tagged = hexToBytes("a3 61 7a 81 a1 61 61 c5 01 61 78 f9 3c 00 61 79 f9 40 00");
    const error = dataError(() => decodeData({ ref: "Point" }, TYPES, tagged, "cbor"));
    expect([error.reason, error.path]).toEqual(["undeclared_tag", '$.z[0]["a"]']);
    const odd = hexToBytes("a3 63 61 2d 62 c5 01 61 78 f9 3c 00 61 79 f9 40 00");
    expect(dataError(() => decodeData({ ref: "Point" }, TYPES, odd, "cbor")).path).toBe('$["a-b"]');
    for (const name of ["", "1a", "a-b", "a\nb", "é"]) {
      expect(dataError(() => decodeData({ ref: "Status" }, TYPES, json(JSON.stringify(name)), "json")).reason, name).toBe(
        "unknown_variant",
      );
      const data = json(`{${JSON.stringify(name)}:1}`);
      expect(dataError(() => decodeData({ ref: "Status" }, TYPES, data, "json")).reason, name).toBe("unknown_variant");
    }
    expect(decodeData({ ref: "Status" }, TYPES, json('"_Later2"'), "json")).toEqual({ kind: "Unknown", name: "_Later2" });
    for (const [input, path] of [
      ["a1 63 5a 65 64 c5 01", "$.Zed"],
      ["a1 63 5a 65 64 81 c1 01", "$.Zed[0]"],
      ["a1 63 5a 65 64 a1 61 78 d8 25 40", '$.Zed["x"]'],
    ] as const) {
      const refused = dataError(() => decodeData({ ref: "Status" }, TYPES, hexToBytes(input), "cbor"));
      expect([refused.reason, refused.path], input).toEqual(["undeclared_tag", path]);
    }
    const untagged = hexToBytes("a1 63 5a 65 64 81 01");
    expect(decodeData({ ref: "Status" }, TYPES, untagged, "cbor")).toEqual({ kind: "Unknown", name: "Zed" });
    expect(dataError(() => decodeData({ ref: "Status" }, TYPES, json('"A\\n"'), "json")).message).not.toContain("\n");
  });

  it("caps an unknown open-enum name at 128 bytes and names a missing field", () => {
    const long = "a".repeat(128);
    const status = (input: Uint8Array, codec: Codec): unknown => decodeData({ ref: "Status" }, TYPES, input, codec);
    expect(status(json(JSON.stringify(long)), "json")).toEqual({ kind: "Unknown", name: long });
    for (const name of [`${long}b`, "a".repeat(65_536)]) {
      expect(dataError(() => status(json(JSON.stringify(name)), "json")).reason).toBe("unknown_variant");
      const cbor = encodeData("any", TYPES, { kind: "text", value: name }, "cbor");
      expect(dataError(() => status(cbor, "cbor")).reason).toBe("unknown_variant");
    }
    expect(dataError(() => status(json('{"Zed":1,"Zee":2}'), "json")).reason).toBe("wrong_type");
    for (const codec of ["cbor", "json"] as const) {
      const input = codec === "cbor" ? hexToBytes("81 a1 61 78 f9 3c 00") : json('[{"x":1}]');
      const missing = dataError(() => decodeData({ list: { ref: "Point" } }, TYPES, input, codec));
      expect([missing.reason, missing.path]).toEqual(["missing_field", "$[0].y"]);
    }
  });

  it("refuses undefined for unit and a depth limit that is not a non-negative integer", () => {
    for (const codec of ["cbor", "json"] as const) {
      expect(encodeError(() => encodeData("unit", TYPES, undefined, codec)).kind).toBe("invalid_value");
      expect(decodeData("unit", TYPES, encodeData("unit", TYPES, null, codec), codec)).toBeNull();
      for (const maxDepth of [Number.NaN, Number.POSITIVE_INFINITY, -1, 1.5]) {
        expect(() => encodeData("u8", TYPES, 1, codec, { maxDepth })).toThrow(RangeError);
        expect(() => decodeData("u8", TYPES, hexToBytes(codec === "cbor" ? "01" : "31"), codec, { maxDepth })).toThrow(RangeError);
      }
    }
  });

  it("names the place of a type the table does not hold", () => {
    const type: ContractType = { list: { map: { ref: "Missing" } } };
    const value = [new Map(), new Map([["b", 1]])];
    const encode = encodeError(() => encodeData(type, TYPES, value, "cbor"));
    expect([encode.kind, encode.path]).toEqual(["unknown_type", '$[1]["b"]']);
    const decode = dataError(() => decodeData(type, TYPES, json('[{},{"b":1}]'), "json"));
    expect([decode.reason, decode.path]).toEqual(["unknown_type", '$[1]["b"]']);
  });

  it("ignores unknown struct fields", () => {
    expect(decodeData({ ref: "Point" }, TYPES, json('{"z":[1,{"a":null}],"x":1,"y":2}'), "json")).toEqual({ x: 1, y: 2 });
    expect(decodeData({ ref: "Point" }, TYPES, hexToBytes("a3 61 7a 80 61 78 f9 3c 00 61 79 f9 40 00"), "cbor")).toEqual({
      x: 1,
      y: 2,
    });
  });

  it("omits optional fields that are null or absent, and writes null for a required option", () => {
    expect(textOf(encodeData({ ref: "Options" }, TYPES, { lang: null }, "json"))).toBe("{}");
    expect(textOf(encodeData({ ref: "Options" }, TYPES, {}, "json"))).toBe("{}");
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
    expect(textOf(encodeData("timestamp", TYPES, new Date(-62_167_219_200_000), "json"))).toBe(
      '"0000-01-01T00:00:00.000Z"',
    );
    expect(dataError(() => decodeData("timestamp", TYPES, json('"2025-02-29T00:00:00.000Z"'), "json")).reason).toBe(
      "not_canonical",
    );
  });

  it("keeps any values as CBOR items in both codecs", () => {
    const item = { kind: "map", entries: [[{ kind: "text", value: "b" }, { kind: "float", value: 0.5 }], [{ kind: "text", value: "a" }, { kind: "nint", value: 0n }]] };
    expect(textOf(encodeData("any", TYPES, item, "json"))).toBe('{"b":0.5,"a":-1}');
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
    expect(decodeData("f32", TYPES, hexToBytes("fb 3f b9 99 99 99 99 99 9a"), "cbor")).toBe(Math.fround(0.1));
    expect(decodeData("f32", TYPES, json("-4.1828256"), "json")).toBe(Math.fround(-4.1828256));
    expect(dataError(() => decodeData("f32", TYPES, json("1e39"), "json")).reason).toBe("out_of_range");
    expect(decodeData("f32", TYPES, json("1e-50"), "json")).toBe(0);
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

  it("caps a run of tags directly around one item at the depth limit", () => {
    const run = (n: number): Uint8Array => hexToBytes(`${"c5 ".repeat(n)}00`);
    expect(() => decodeData("any", TYPES, run(16), "cbor")).not.toThrow();
    const runs = hexToBytes(`${"c5 ".repeat(16)}81 ${"c5 ".repeat(16)}00`);
    expect(() => decodeData("any", TYPES, runs, "cbor")).not.toThrow();
    expect(dataError(() => decodeData("any", TYPES, run(17), "cbor")).reason).toBe("too_deep");
    expect(dataError(() => decodeData("any", TYPES, run(100_000), "cbor")).reason).toBe("too_deep");
    let tagged: CborItem = { kind: "uint", value: 0n };
    for (let i = 0; i < 16; i++) tagged = { kind: "tag", tag: 5n, item: tagged };
    expect(() => encodeData("any", TYPES, tagged, "cbor")).not.toThrow();
    tagged = { kind: "tag", tag: 5n, item: tagged };
    expect(encodeError(() => encodeData("any", TYPES, tagged, "cbor")).kind).toBe("too_deep");
  });

  it("names the place of an error a scalar writer finds", () => {
    expect(encodeError(() => encodeData({ ref: "Point" }, TYPES, { x: Number.NaN, y: 1 }, "json")).path).toBe("$.x");
    expect(encodeError(() => encodeData("f64", TYPES, Number.NaN, "json")).path).toBe("$");
    expect(encodeError(() => encodeData({ list: "string" }, TYPES, ["a", "\ud800"], "cbor")).path).toBe("$[1]");
  });

  it("keeps struct fields named like Object.prototype members as own fields", () => {
    for (const codec of ["cbor", "json"] as const) {
      const cbor = "a1 69 5f 5f 70 72 6f 74 6f 5f 5f a2 61 78 f9 3c 00 61 79 f9 40 00";
      const bytes = codec === "json" ? json('{"__proto__":{"x":1,"y":2}}') : hexToBytes(cbor);
      const value = decodeData({ ref: "Reserved" }, TYPES, bytes, codec) as Record<string, unknown>;
      expect(Object.getPrototypeOf(value), codec).toBe(Object.prototype);
      expect(Object.keys(value), codec).toEqual(["__proto__", "constructor"]);
      expect(value["constructor"], codec).toBeNull();
      expect(textOf(encodeData({ ref: "Reserved" }, TYPES, value, "json")), codec).toBe('{"__proto__":{"x":1,"y":2}}');
    }
    const own = Object.fromEntries([["__proto__", { x: 1, y: 2 }]]);
    expect(textOf(encodeData({ ref: "Reserved" }, TYPES, own, "json"))).toBe('{"__proto__":{"x":1,"y":2}}');
    expect(encodeError(() => encodeData({ ref: "Reserved" }, TYPES, {}, "json")).path).toBe("$.__proto__");
  });

  it("refuses an alias that never reaches a type", () => {
    expect(encodeError(() => encodeData({ ref: "Loop" }, TYPES, 1, "cbor")).kind).toBe("unknown_type");
    for (const codec of ["cbor", "json"] as const) {
      const one = hexToBytes(codec === "cbor" ? "01" : "31");
      for (const ref of ["Nested", "PingA", "PingB", "Twice"]) {
        expect(encodeError(() => encodeData({ ref }, TYPES, 5, codec)).kind, ref).toBe("unknown_type");
        expect(dataError(() => decodeData({ ref }, TYPES, one, codec)).reason, ref).toBe("unknown_type");
        const list = hexToBytes(codec === "cbor" ? "81 01" : "5b 31 5d");
        expect(dataError(() => decodeData({ list: { ref } }, TYPES, list, codec)).reason).toBe("unknown_type");
        both({ ref }, null);
      }
    }
    expect(encodeError(() => encodeData({ ref: "Twice" }, TYPES, 5, "cbor")).message).toContain("loops back to itself");
  });

  it("reads null through an option whose type never reaches one", () => {
    both({ ref: "Nested" }, null);
    both({ option: { ref: "Missing" } }, null);
    expect(encodeError(() => encodeData({ option: { ref: "Missing" } }, TYPES, 1, "cbor")).kind).toBe("unknown_type");
  });

  it("decodes tagged types inside an option", () => {
    const extra = { kind: "tag", tag: 5n, item: { kind: "uint", value: 1n } };
    const event = { at: new Date(1000), id: UUID, extra };
    const encoded = encodeData({ ref: "Event" }, TYPES, event, "cbor");
    expect(decodeData({ ref: "Event" }, TYPES, encoded, "cbor")).toEqual(event);
    both({ ref: "Event" }, { at: null, id: null, extra: null });
    expect(decodeData({ option: "timestamp" }, TYPES, hexToBytes("c1 01"), "cbor")).toEqual(new Date(1000));
    expect(dataError(() => decodeData({ option: "u8" }, TYPES, hexToBytes("c1 01"), "cbor")).reason).toBe(
      "undeclared_tag",
    );
  });

  it("accepts indefinite lengths in data sections", () => {
    const cbor = (type: ContractType, hex: string): unknown => decodeData(type, TYPES, hexToBytes(hex), "cbor");
    expect(cbor({ list: "u8" }, "9f 01 02 ff")).toEqual([1, 2]);
    expect(cbor("string", "7f 61 61 62 62 63 ff")).toBe("abc");
    expect(cbor("bytes", "5f 41 01 42 02 03 ff")).toEqual(Uint8Array.of(1, 2, 3));
    expect(cbor({ map: "u8" }, "bf 61 61 01 ff")).toEqual(new Map([["a", 1]]));
    expect(cbor({ ref: "Point" }, "bf 61 79 f9 3c 00 61 78 f9 3c 00 ff")).toEqual({ x: 1, y: 1 });
    expect(cbor("any", "9f ff")).toEqual({ kind: "array", items: [] });
    const broken = ["9f 01", "7f 41 01 ff", "5f 61 61 ff", "7f 7f ff ff", "7f 61 c3 61 a9 ff", "bf 61 61 ff", "5f"];
    broken.push("5f 5d ff", "7f 7c ff", "5f 5c 00 ff", "7f 7e ff");
    for (const input of broken) {
      expect(dataError(() => cbor("any", input)).reason, input).toBe("malformed");
    }
    expect(protocolError(() => decodeHeader(hexToBytes("a1 01 9f ff"))).reason).toBe("indefinite_length");
  });

  it("passes duplicate keys of any through and refuses them in typed maps and structs", () => {
    expect(decodeData("any", TYPES, hexToBytes("a2 61 61 01 61 61 02"), "cbor")).toEqual({
      kind: "map",
      entries: [
        [{ kind: "text", value: "a" }, { kind: "uint", value: 1n }],
        [{ kind: "text", value: "a" }, { kind: "uint", value: 2n }],
      ],
    });
    for (const [type, hex] of [
      [{ map: "u8" }, "a2 61 61 01 78 01 61 02"],
      [{ ref: "Point" }, "bf 61 78 01 61 78 01 ff"],
    ] as const) {
      expect(dataError(() => decodeData(type, TYPES, hexToBytes(hex), "cbor")).reason, hex).toBe("duplicate_key");
    }
  });

  it("reads JSON -0 as a float", () => {
    expect(dataError(() => decodeData("u8", TYPES, json("-0"), "json")).reason).toBe("wrong_type");
    expect(decodeData("any", TYPES, json("-0"), "json")).toEqual({ kind: "float", value: -0 });
    expect(decodeData("any", TYPES, json("0"), "json")).toEqual({ kind: "uint", value: 0n });
  });

  it("refuses an any that is not an item and a unit variant with a value", () => {
    for (const codec of ["cbor", "json"] as const) {
      const a = { kind: "text", value: "a" };
      for (const value of [
        5,
        undefined,
        null,
        { kind: "array", items: [undefined] },
        { kind: "map", entries: [[a, undefined]] },
        { kind: "map", entries: [[undefined, a]] },
      ]) {
        expect(encodeError(() => encodeData("any", TYPES, value, codec)).kind).toBe("invalid_value");
      }
      const unit = encodeError(() => encodeData({ ref: "Closed" }, TYPES, { kind: "A", value: 3 }, codec));
      expect(unit.kind).toBe("invalid_value");
    }
    expect(encodeError(() => encodeData("any", TYPES, { kind: "nope" }, "cbor")).kind).toBe("invalid_value");
    expect(encodeError(() => encodeData("any", TYPES, undefined, "cbor")).kind).toBe("invalid_value");
  });

  it("refuses a typed tag where the limit admits no tag run", () => {
    for (const [type, value] of [
      ["timestamp", new Date(0)],
      ["uuid", UUID],
    ] as const) {
      expect(encodeError(() => encodeData(type, TYPES, value, "cbor", { maxDepth: 0 })).kind, type).toBe("too_deep");
    }
  });

  it("copies decoded any bytes out of the input", () => {
    const input = hexToBytes("42 01 02");
    const item = decodeData("any", TYPES, input, "cbor") as { value: Uint8Array };
    input[1] = 9;
    expect([...item.value]).toEqual([1, 2]);
  });
});
