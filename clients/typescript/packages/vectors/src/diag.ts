// CBOR diagnostic notation (RFC 8949 section 8), the basic subset the vectors use, plus the
// pattern extensions of vectors/README.md: `*` (any value), `...` (map may have more keys) and
// `$name` (a captured value).

export type Diag =
  | { kind: "uint"; value: bigint }
  | { kind: "nint"; value: bigint } // -1 - value
  | { kind: "float"; value: number }
  | { kind: "text"; value: string }
  | { kind: "bytes"; value: Uint8Array }
  | { kind: "array"; items: Diag[] }
  | { kind: "map"; entries: [Diag, Diag][]; open: boolean }
  | { kind: "tag"; tag: bigint; value: Diag }
  | { kind: "bool"; value: boolean }
  | { kind: "null" }
  | { kind: "any" }
  | { kind: "ref"; name: string };

export class DiagError extends Error {
  constructor(
    message: string,
    readonly offset: number,
  ) {
    super(`at offset ${offset}: ${message}`);
  }
}

const MAX_U64 = (1n << 64n) - 1n;

/** Parses one item; trailing input other than whitespace is an error. */
export function parse(input: string): Diag {
  const p = new Parser(input);
  const item = p.item();
  p.skipWs();
  if (p.pos !== input.length) throw p.error("unexpected trailing input");
  return item;
}

/** Every `$name` referenced in the item, in order of appearance. */
export function refs(d: Diag): string[] {
  switch (d.kind) {
    case "ref":
      return [d.name];
    case "array":
      return d.items.flatMap(refs);
    case "map":
      return d.entries.flatMap(([k, v]) => [...refs(k), ...refs(v)]);
    case "tag":
      return refs(d.value);
    default:
      return [];
  }
}

/** Number of pattern elements (`*`, `$name`, open maps) in the item. */
export function patternCount(d: Diag): number {
  switch (d.kind) {
    case "any":
    case "ref":
      return 1;
    case "array":
      return d.items.reduce((n, i) => n + patternCount(i), 0);
    case "map":
      return d.entries.reduce((n, [k, v]) => n + patternCount(k) + patternCount(v), d.open ? 1 : 0);
    case "tag":
      return patternCount(d.value);
    default:
      return 0;
  }
}

/** Parses hex digits, ignoring spaces. */
export function hexToBytes(hex: string): Uint8Array {
  const digits = hex.replace(/\s+/g, "");
  if (digits.length % 2 !== 0 || /[^0-9a-fA-F]/.test(digits)) {
    throw new Error(`invalid hex: ${hex}`);
  }
  const out = new Uint8Array(digits.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(digits.slice(2 * i, 2 * i + 2), 16);
  return out;
}

class Parser {
  pos = 0;

  constructor(private readonly src: string) {}

  error(message: string): DiagError {
    return new DiagError(message, this.pos);
  }

  skipWs(): void {
    while (this.pos < this.src.length && /\s/.test(this.src[this.pos] ?? "")) this.pos++;
  }

  eat(ch: string): boolean {
    this.skipWs();
    if (this.src[this.pos] === ch) {
      this.pos++;
      return true;
    }
    return false;
  }

  expect(ch: string): void {
    if (!this.eat(ch)) throw this.error(`expected \`${ch}\``);
  }

  startsWith(word: string): boolean {
    return this.src.startsWith(word, this.pos);
  }

  item(): Diag {
    this.skipWs();
    const c = this.src[this.pos];
    if (c === undefined) throw this.error("unexpected end of input");
    if (c === "[") return this.array();
    if (c === "{") return this.map();
    if (c === '"') return { kind: "text", value: this.text() };
    if (this.startsWith("h'")) return this.bytes();
    if (c === "*") {
      this.pos++;
      return { kind: "any" };
    }
    if (c === "$") return this.reference();
    if (c === "-" || (c >= "0" && c <= "9")) return this.numberOrTag();
    return this.word();
  }

  array(): Diag {
    this.expect("[");
    const items: Diag[] = [];
    if (this.eat("]")) return { kind: "array", items };
    for (;;) {
      items.push(this.item());
      if (this.eat("]")) return { kind: "array", items };
      this.expect(",");
    }
  }

  map(): Diag {
    this.expect("{");
    const entries: [Diag, Diag][] = [];
    if (this.eat("}")) return { kind: "map", entries, open: false };
    for (;;) {
      this.skipWs();
      if (this.startsWith("...")) {
        this.pos += 3;
        this.expect("}");
        return { kind: "map", entries, open: true };
      }
      const key = this.item();
      this.expect(":");
      entries.push([key, this.item()]);
      if (this.eat("}")) return { kind: "map", entries, open: false };
      this.expect(",");
    }
  }

  text(): string {
    // Diagnostic-notation text strings use the JSON string syntax.
    const start = this.pos;
    this.pos++;
    while (this.pos < this.src.length && this.src[this.pos] !== '"') {
      this.pos += this.src[this.pos] === "\\" ? 2 : 1;
    }
    if (this.pos >= this.src.length) throw this.error("unterminated string");
    this.pos++;
    try {
      return JSON.parse(this.src.slice(start, this.pos)) as string;
    } catch {
      throw new DiagError("invalid string", start);
    }
  }

  bytes(): Diag {
    this.pos += 2;
    const end = this.src.indexOf("'", this.pos);
    if (end < 0) throw this.error("unterminated byte string");
    const value = hexToBytes(this.src.slice(this.pos, end));
    this.pos = end + 1;
    return { kind: "bytes", value };
  }

  reference(): Diag {
    this.pos++;
    const m = /^[a-z0-9_]+/.exec(this.src.slice(this.pos));
    if (!m) throw this.error("empty `$` reference");
    this.pos += m[0].length;
    return { kind: "ref", name: m[0] };
  }

  numberOrTag(): Diag {
    if (this.startsWith("-Infinity")) {
      this.pos += 9;
      return { kind: "float", value: -Infinity };
    }
    const m = /^-?[0-9][0-9.eE+-]*/.exec(this.src.slice(this.pos));
    if (!m) throw this.error("invalid number");
    const token = m[0];
    this.pos += token.length;
    const isFloat = /[.eE]/.test(token);
    if (!isFloat && this.src[this.pos] === "(") {
      this.pos++;
      const value = this.item();
      this.expect(")");
      return { kind: "tag", tag: BigInt(token), value };
    }
    if (isFloat) {
      const value = Number(token);
      if (Number.isNaN(value)) throw this.error("invalid float");
      return { kind: "float", value };
    }
    const n = BigInt(token);
    if (n >= 0n) {
      if (n > MAX_U64) throw this.error("integer out of CBOR range");
      return { kind: "uint", value: n };
    }
    if (-1n - n > MAX_U64) throw this.error("integer out of CBOR range");
    return { kind: "nint", value: -1n - n };
  }

  word(): Diag {
    const words: [string, Diag][] = [
      ["true", { kind: "bool", value: true }],
      ["false", { kind: "bool", value: false }],
      ["null", { kind: "null" }],
      ["NaN", { kind: "float", value: NaN }],
      ["Infinity", { kind: "float", value: Infinity }],
    ];
    for (const [word, value] of words) {
      if (this.startsWith(word)) {
        this.pos += word.length;
        return value;
      }
    }
    throw this.error("unexpected character");
  }
}
