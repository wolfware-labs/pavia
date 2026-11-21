import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { type Diag, hexToBytes, parse } from "@pavia/vectors/diag";
import type { HeaderMap, HeaderValue } from "../src/index.js";

const VECTORS = fileURLToPath(new URL("../../../../../vectors/", import.meta.url));

export type Json = Record<string, unknown>;

export interface Case {
  readonly id: string;
  readonly c: Json;
}

export function cases(kind: string): Case[] {
  const out: Case[] = [];
  for (const file of readdirSync(VECTORS + kind).filter((f) => f.endsWith(".json")).sort()) {
    const doc = JSON.parse(readFileSync(`${VECTORS}${kind}/${file}`, "utf8")) as { cases: Json[] };
    for (const c of doc.cases) out.push({ id: `${kind}/${file}#${String(c.id)}`, c });
  }
  return out;
}

export { hexToBytes };

export function value(d: Diag): HeaderValue {
  switch (d.kind) {
    case "uint":
    case "nint":
    case "text":
    case "bytes":
    case "bool":
    case "float":
      return { kind: d.kind, value: d.value } as HeaderValue;
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

export function headerOf(diag: string): HeaderMap {
  const v = value(parse(diag));
  if (v.kind !== "map") throw new Error("header is not a map");
  return v.entries;
}

export function valueOf(diag: string): HeaderValue {
  return value(parse(diag));
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
