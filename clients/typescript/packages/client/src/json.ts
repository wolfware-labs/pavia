import { EncodeError } from "./errors.js";

/**
 * One JSON value as read from a data section (spec 12.3). A number keeps its text, so the typed
 * layer can tell `1`, `1.0` and `1e0` apart and read integers past 2^53 exactly.
 */
export type JsonValue =
  | { readonly kind: "null" }
  | { readonly kind: "bool"; readonly value: boolean }
  | { readonly kind: "number"; readonly text: string }
  | { readonly kind: "string"; readonly value: string }
  | { readonly kind: "array"; readonly items: readonly JsonValue[] }
  | { readonly kind: "object"; readonly entries: readonly (readonly [string, JsonValue])[] };

/** Why a JSON data section was refused. */
export type JsonErrorReason = "malformed" | "too_deep" | "duplicate_key";

const UTF8_DECODER = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
const NUMBER = /-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?/y;

/**
 * Parses `bytes` as one JSON text (RFC 8259) in UTF-8, refusing invalid UTF-8, a byte order mark,
 * NaN and Infinity, duplicate object keys, lone surrogate escapes and nesting deeper than
 * `maxDepth`, where a top-level array or object has depth 1.
 *
 * @throws the error `fail` returns for the first rule that fails.
 */
export function parseJson(
  bytes: Uint8Array,
  maxDepth: number,
  fail: (reason: JsonErrorReason) => Error,
): JsonValue {
  let text: string;
  try {
    text = UTF8_DECODER.decode(bytes);
  } catch {
    throw fail("malformed");
  }
  const parser = new Parser(text, maxDepth, fail);
  parser.space();
  const value = parser.value(0);
  parser.space();
  if (parser.pos < text.length) throw fail("malformed");
  return value;
}

class Parser {
  pos = 0;

  constructor(
    private readonly text: string,
    private readonly maxDepth: number,
    private readonly fail: (reason: JsonErrorReason) => Error,
  ) {}

  space(): void {
    for (;;) {
      const c = this.text.charCodeAt(this.pos);
      if (c !== 0x20 && c !== 0x09 && c !== 0x0a && c !== 0x0d) return;
      this.pos++;
    }
  }

  value(depth: number): JsonValue {
    const c = this.text[this.pos];
    switch (c) {
      case "{":
        return this.object(this.deeper(depth));
      case "[":
        return this.array(this.deeper(depth));
      case '"':
        return { kind: "string", value: this.string() };
      case "t":
        return this.literal("true", { kind: "bool", value: true });
      case "f":
        return this.literal("false", { kind: "bool", value: false });
      case "n":
        return this.literal("null", { kind: "null" });
      default: {
        NUMBER.lastIndex = this.pos;
        const match = NUMBER.exec(this.text);
        if (match === null) throw this.fail("malformed");
        this.pos += match[0].length;
        return { kind: "number", text: match[0] };
      }
    }
  }

  private deeper(depth: number): number {
    if (depth + 1 > this.maxDepth) throw this.fail("too_deep");
    return depth + 1;
  }

  private literal(word: string, value: JsonValue): JsonValue {
    if (!this.text.startsWith(word, this.pos)) throw this.fail("malformed");
    this.pos += word.length;
    return value;
  }

  private array(depth: number): JsonValue {
    this.pos++;
    const items: JsonValue[] = [];
    this.space();
    if (this.text[this.pos] === "]") {
      this.pos++;
      return { kind: "array", items };
    }
    for (;;) {
      this.space();
      items.push(this.value(depth));
      this.space();
      const c = this.text[this.pos++];
      if (c === "]") return { kind: "array", items };
      if (c !== ",") throw this.fail("malformed");
    }
  }

  private object(depth: number): JsonValue {
    this.pos++;
    const entries: (readonly [string, JsonValue])[] = [];
    const keys = new Set<string>();
    this.space();
    if (this.text[this.pos] === "}") {
      this.pos++;
      return { kind: "object", entries };
    }
    for (;;) {
      this.space();
      if (this.text[this.pos] !== '"') throw this.fail("malformed");
      const key = this.string();
      if (keys.has(key)) throw this.fail("duplicate_key");
      keys.add(key);
      this.space();
      if (this.text[this.pos++] !== ":") throw this.fail("malformed");
      this.space();
      entries.push([key, this.value(depth)]);
      this.space();
      const c = this.text[this.pos++];
      if (c === "}") return { kind: "object", entries };
      if (c !== ",") throw this.fail("malformed");
    }
  }

  private string(): string {
    this.pos++;
    let out = "";
    let start = this.pos;
    for (;;) {
      const c = this.text.charCodeAt(this.pos);
      if (Number.isNaN(c) || c < 0x20) throw this.fail("malformed");
      if (c === 0x22) {
        out += this.text.slice(start, this.pos);
        this.pos++;
        return out;
      }
      if (c !== 0x5c) {
        this.pos++;
        continue;
      }
      out += this.text.slice(start, this.pos);
      out += this.escape();
      start = this.pos;
    }
  }

  private escape(): string {
    const c = this.text[this.pos + 1];
    this.pos += 2;
    switch (c) {
      case '"':
      case "\\":
      case "/":
        return c;
      case "b":
        return "\b";
      case "f":
        return "\f";
      case "n":
        return "\n";
      case "r":
        return "\r";
      case "t":
        return "\t";
      case "u": {
        const unit = this.hex4();
        if (unit >= 0xdc00 && unit <= 0xdfff) throw this.fail("malformed");
        if (unit < 0xd800 || unit > 0xdbff) return String.fromCharCode(unit);
        if (!this.text.startsWith("\\u", this.pos)) throw this.fail("malformed");
        this.pos += 2;
        const low = this.hex4();
        if (low < 0xdc00 || low > 0xdfff) throw this.fail("malformed");
        return String.fromCharCode(unit, low);
      }
      default:
        throw this.fail("malformed");
    }
  }

  private hex4(): number {
    const digits = this.text.slice(this.pos, this.pos + 4);
    if (!/^[0-9a-fA-F]{4}$/.test(digits)) throw this.fail("malformed");
    this.pos += 4;
    return Number.parseInt(digits, 16);
  }
}

const LONE_SURROGATE = /\p{Surrogate}/u;
const SHORT_ESCAPES: Readonly<Record<number, string>> = {
  0x22: '\\"',
  0x5c: "\\\\",
  0x08: "\\b",
  0x0c: "\\f",
  0x0a: "\\n",
  0x0d: "\\r",
  0x09: "\\t",
};

/**
 * `text` as a JSON string in RFC 8785 form: `"`, `\` and the control characters escaped, with
 * `\b \f \n \r \t` and lowercase `\u00xx` for the others, and every other character as itself.
 *
 * @throws {@link EncodeError} `lone_surrogate` for text with no UTF-8 form.
 */
export function jsonString(text: string): string {
  if (LONE_SURROGATE.test(text)) {
    throw new EncodeError("lone_surrogate", "text holds a lone UTF-16 surrogate, which has no UTF-8 form");
  }
  let out = '"';
  let start = 0;
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i);
    if (c >= 0x20 && c !== 0x22 && c !== 0x5c) continue;
    out += text.slice(start, i) + (SHORT_ESCAPES[c] ?? `\\u${c.toString(16).padStart(4, "0")}`);
    start = i + 1;
  }
  return `${out}${text.slice(start)}"`;
}

/**
 * `x` as a JSON number in RFC 8785 form, which is the ECMAScript `Number` to string conversion:
 * `2`, `1.5`, `1e+21`, and `0` for negative zero.
 *
 * @throws {@link EncodeError} `not_json` for NaN and the infinities, which JSON cannot hold.
 */
export function jsonNumber(x: number): string {
  if (!Number.isFinite(x)) throw new EncodeError("not_json", `${x} has no JSON form`);
  return String(x);
}

/** Orders strings by their code points, which is the order of their UTF-8 bytes. */
export function compareCodePoints(a: string, b: string): number {
  const n = Math.min(a.length, b.length);
  for (let i = 0; i < n; i++) {
    const x = a.charCodeAt(i);
    const y = b.charCodeAt(i);
    if (x === y) continue;
    const xs = x >= 0xd800 && x <= 0xdfff;
    const ys = y >= 0xd800 && y <= 0xdfff;
    if (xs !== ys) return xs ? 1 : -1;
    return x - y;
  }
  return a.length - b.length;
}
