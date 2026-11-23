import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { type Diag, hexToBytes, parse } from "@pavia/vectors/diag";
import { expect } from "vitest";
import {
  type CborItem,
  type CborMap,
  DataError,
  DEFAULT_MAX_FRAME,
  decodeFrame,
  decodeMessage,
  EncodeError,
  encodeMessages,
  type Frame,
  ProtocolError,
  toRawFrame,
  type Welcome,
} from "../src/index.js";

const VECTORS = fileURLToPath(new URL("../../../../../vectors/", import.meta.url));

export type Json = Record<string, unknown>;

export interface Case {
  readonly id: string;
  readonly c: Json;
}

export function files(kind: string): { readonly file: string; readonly doc: Json }[] {
  return readdirSync(VECTORS + kind)
    .filter((f) => f.endsWith(".json"))
    .sort()
    .map((file) => ({ file, doc: JSON.parse(readFileSync(`${VECTORS}${kind}/${file}`, "utf8")) as Json }));
}

export function cases(kind: string): Case[] {
  const out: Case[] = [];
  for (const { file, doc } of files(kind)) {
    for (const c of doc["cases"] as Json[]) out.push({ id: `${kind}/${file}#${String(c["id"])}`, c });
  }
  return out;
}

export { hexToBytes };

export function value(d: Diag): CborItem {
  switch (d.kind) {
    case "uint":
    case "nint":
    case "text":
    case "bytes":
    case "bool":
    case "float":
      return { kind: d.kind, value: d.value } as CborItem;
    case "null":
      return { kind: "null" };
    case "array":
      return { kind: "array", items: d.items.map(value) };
    case "map":
      return { kind: "map", entries: d.entries.map(([k, v]) => [value(k), value(v)] as const) };
    default:
      throw new Error(`no header value for diagnostic ${d.kind}`);
  }
}

export function headerOf(diag: string): CborMap {
  const v = value(parse(diag));
  if (v.kind !== "map") throw new Error("header is not a map");
  return v.entries;
}

export function valueOf(diag: string): CborItem {
  return value(parse(diag));
}

function thrown(f: () => unknown): unknown {
  try {
    f();
  } catch (error) {
    return error;
  }
  throw new Error("expected an error");
}

export function protocolError(f: () => unknown): ProtocolError {
  const error = thrown(f);
  expect(error).toBeInstanceOf(ProtocolError);
  return error as ProtocolError;
}

export function encodeError(f: () => unknown): EncodeError {
  const error = thrown(f);
  expect(error).toBeInstanceOf(EncodeError);
  return error as EncodeError;
}

export function dataError(f: () => unknown): DataError {
  const error = thrown(f);
  expect(error).toBeInstanceOf(DataError);
  return error as DataError;
}

export function concat(parts: readonly Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
}

export const HB = 50;
export const IDLE = 200;
export const CONTRACT = new Uint8Array(32).fill(7);

export function welcome(fields: Partial<Welcome> = {}): Welcome {
  return {
    type: "WELCOME",
    version: 1n,
    codec: "cbor",
    caps: [],
    session: "s1",
    limits: { maxCalls: 100n, maxFrame: BigInt(DEFAULT_MAX_FRAME), maxSubs: 256n, maxDgram: 1100n, window: 262_144n },
    hb: BigInt(HB),
    idle: BigInt(IDLE),
    ...fields,
  };
}

export function message(frame: Frame, stream = 0): Uint8Array {
  return encodeMessages([{ stream, frame: toRawFrame(frame) }], DEFAULT_MAX_FRAME)[0] as Uint8Array;
}

export function framesOf(data: Uint8Array): Frame[] {
  return decodeMessage(data, DEFAULT_MAX_FRAME).map(({ frame }) => decodeFrame(frame));
}
