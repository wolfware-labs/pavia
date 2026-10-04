// The `json` codec of spec 12.3, written as a test oracle: renders a value of a given contract
// type as the canonical `json` text (compact, RFC 8785 strings and numbers, struct members in
// declaration order, `map` members sorted by UTF-8 key bytes).

import type { Diag } from "./diag.js";

type Type = string | Record<string, unknown>;
type Types = Record<string, Type>;

const utf8 = new TextEncoder();

function compareUtf8(a: string, b: string): number {
  const x = utf8.encode(a);
  const y = utf8.encode(b);
  for (let i = 0; i < Math.min(x.length, y.length); i++) {
    if (x[i] !== y[i]) return (x[i] ?? 0) - (y[i] ?? 0);
  }
  return x.length - y.length;
}

function textOf(d: Diag): string {
  if (d.kind !== "text") throw new Error(`expected text, got ${d.kind}`);
  return d.value;
}

function base64url(bytes: Uint8Array): string {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function number(d: Diag): number {
  if (d.kind === "float") return d.value;
  if (d.kind === "uint") return Number(d.value);
  if (d.kind === "nint") return -1 - Number(d.value);
  throw new Error(`expected a number, got ${d.kind}`);
}

/** Renders `value`, whose contract type is `ty`, as canonical `json` text. */
export function render(ty: Type, value: Diag, types: Types): string {
  if (typeof ty === "string") return primitive(ty, value);
  const [kind, arg] = Object.entries(ty)[0] as [string, unknown];
  switch (kind) {
    case "ref": {
      const def = types[arg as string];
      if (def === undefined) throw new Error(`unknown type ${String(arg)}`);
      return render(def, value, types);
    }
    case "alias":
      return render(arg as Type, value, types);
    case "option":
      return value.kind === "null" ? "null" : render(arg as Type, value, types);
    case "list":
      if (value.kind !== "array") throw new Error("list expects an array");
      return `[${value.items.map((i) => render(arg as Type, i, types)).join(",")}]`;
    case "map": {
      if (value.kind !== "map") throw new Error("map expects a map");
      const members = value.entries
        .map(([k, v]) => [textOf(k), v] as const)
        .sort((a, b) => compareUtf8(a[0], b[0]));
      return `{${members.map(([k, v]) => `${JSON.stringify(k)}:${render(arg as Type, v, types)}`).join(",")}}`;
    }
    case "struct": {
      if (value.kind !== "map") throw new Error("struct expects a map");
      const fields = (arg as { fields: { name: string; type: Type }[] }).fields;
      const parts: string[] = [];
      for (const f of fields) {
        const entry = value.entries.find(([k]) => k.kind === "text" && k.value === f.name);
        if (entry) parts.push(`${JSON.stringify(f.name)}:${render(f.type, entry[1], types)}`);
      }
      return `{${parts.join(",")}}`;
    }
    case "enum": {
      const variants = (arg as { variants: { name: string; type?: Type }[] }).variants;
      if (value.kind === "text") return JSON.stringify(value.value);
      if (value.kind !== "map" || value.entries.length !== 1) throw new Error("enum expects text or a one-entry map");
      const [k, v] = value.entries[0] as [Diag, Diag];
      const name = textOf(k);
      const variant = variants.find((x) => x.name === name);
      if (!variant?.type) throw new Error(`unknown data variant ${name}`);
      return `{${JSON.stringify(name)}:${render(variant.type, v, types)}}`;
    }
    default:
      throw new Error(`type ${kind} not supported by the oracle`);
  }
}

function primitive(name: string, v: Diag): string {
  switch (name) {
    case "unit":
      return "null";
    case "bool":
      if (v.kind !== "bool") throw new Error("bool expected");
      return String(v.value);
    case "u8":
    case "u16":
    case "u32":
    case "i8":
    case "i16":
    case "i32":
      return String(number(v));
    case "u64":
    case "i64":
      if (v.kind === "uint") return `"${v.value}"`;
      if (v.kind === "nint") return `"${-1n - v.value}"`;
      throw new Error("integer expected");
    case "f32":
    case "f64": {
      const x = number(v);
      if (!Number.isFinite(x)) throw new Error("NaN and infinities have no json form");
      return JSON.stringify(x); // RFC 8785 numbers are the ECMAScript format
    }
    case "string":
      return JSON.stringify(textOf(v));
    case "bytes":
      if (v.kind !== "bytes") throw new Error("bytes expected");
      return `"${base64url(v.value)}"`;
    case "timestamp": {
      if (v.kind !== "tag" || v.tag !== 1n) throw new Error("timestamp is tag 1");
      return `"${new Date(Math.round(number(v.value) * 1000)).toISOString()}"`;
    }
    case "uuid": {
      if (v.kind !== "tag" || v.tag !== 37n || v.value.kind !== "bytes" || v.value.value.length !== 16) {
        throw new Error("uuid is tag 37 over 16 bytes");
      }
      const hex = [...v.value.value].map((b) => b.toString(16).padStart(2, "0")).join("");
      return `"${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}"`;
    }
    default:
      throw new Error(`${name} not supported by the oracle`);
  }
}
