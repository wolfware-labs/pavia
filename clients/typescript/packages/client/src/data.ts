import { ByteWriter, compareBytes } from "./bytes.js";
import {
  ARRAY,
  BYTES,
  type CborItem,
  checkLimit,
  decodeCborItem,
  hasF32Form,
  MAP,
  MAX_U64,
  TAG,
  writeCborItem,
  writeFloat,
  writeHead,
  writeText,
  writeUtf8,
} from "./cbor.js";
import { type DataErrorReason, DataError, EncodeError, type EncodeErrorKind, type HeaderErrorReason } from "./errors.js";
import { compareCodePoints, type JsonErrorReason, type JsonValue, jsonNumber, jsonString, parseJson } from "./json.js";
import {
  base64url,
  bytesOfBase64url,
  epochSeconds,
  integerItem,
  integerOfItem,
  integerOfText,
  MAX_MILLIS,
  MIN_MILLIS,
  millisOfEpochSeconds,
  millisOfRfc3339,
  rfc3339,
  uuidOfText,
  uuidText,
} from "./scalars.js";
import type { ContractType, EnumValue, Field, NamedType, Primitive, Types, Variant } from "./types.js";
import { utf8Of } from "./utf8.js";

/** The data codec of a session (spec 6.1): `cbor` or `json`. */
export type Codec = "cbor" | "json";

/** The limits applied to a data section (spec 12.3). */
export interface DataLimits {
  /** The deepest nesting of arrays and maps; a top-level array or map has depth 1. Default 16. */
  readonly maxDepth: number;
}

/** The default of spec 12.3: depth 16, as for headers. */
export const DEFAULT_DATA_LIMITS: DataLimits = { maxDepth: 16 };

const TIMESTAMP_TAG = 1n;
const UUID_TAG = 37n;

const INTEGER_RANGE: Readonly<Partial<Record<Primitive, readonly [bigint, bigint]>>> = {
  i8: [-(2n ** 7n), 2n ** 7n - 1n],
  i16: [-(2n ** 15n), 2n ** 15n - 1n],
  i32: [-(2n ** 31n), 2n ** 31n - 1n],
  i64: [-(2n ** 63n), 2n ** 63n - 1n],
  u8: [0n, 2n ** 8n - 1n],
  u16: [0n, 2n ** 16n - 1n],
  u32: [0n, 2n ** 32n - 1n],
  u64: [0n, MAX_U64],
};

/**
 * Encodes `value` of contract type `type` as a data section in `codec` (spec 12.3), in the one
 * encoding the spec allows: struct fields in declaration order, `map` keys sorted (by encoded
 * bytes in `cbor`, by UTF-8 bytes in `json`), shortest integers and floats, a timestamp as whole
 * seconds or the narrowest float that keeps its milliseconds, and compact RFC 8785 `json`.
 *
 * Values are `null` for `unit` and an absent `option`, `number` for 8 to 32-bit integers and
 * floats, `bigint` for `i64` and `u64`, `Uint8Array` for `bytes`, `Date` for `timestamp`, a
 * lowercase hyphenated string for `uuid`, arrays for `list` and `tuple`, `Map` for `map`, objects
 * keyed by field name for structs, {@link EnumValue} for enums and a {@link CborItem} for `any`.
 * `unit` takes `null` only, so a function that returns nothing has its `undefined` mapped to
 * `null` before it is encoded; an `option` alone reads `undefined` as absent.
 *
 * @throws {@link EncodeError} when `value` does not fit `type` or a receiver within `limits` would
 * refuse it, with the place in `path` (`$.sent_at`, `$[2]`).
 * @throws RangeError when `limits.maxDepth` is not a non-negative integer.
 */
export function encodeData(
  type: ContractType,
  types: Types,
  value: unknown,
  codec: Codec,
  limits: DataLimits = DEFAULT_DATA_LIMITS,
): Uint8Array {
  checkLimit("maxDepth", limits.maxDepth);
  try {
    if (codec === "cbor") {
      const out = new ByteWriter();
      new CborEncoder(types, limits.maxDepth).write(out, type, value, 0);
      return out.finish();
    }
    const out = new JsonText();
    new JsonEncoder(types, limits.maxDepth).write(out, type, value, 0);
    return utf8Of(out.text);
  } catch (error) {
    if (error instanceof UnknownType) throw new EncodeError("unknown_type", error.message, error.path);
    if (error instanceof EncodeError && error.path === undefined) throw placed(error, "$");
    throw error;
  }
}

/**
 * Decodes a data section in `codec` as contract type `type` (spec 12.3), accepting struct fields
 * and map keys in any order and any valid encoding, and returning values in the forms
 * {@link encodeData} takes. Unknown struct fields are ignored; a variant an open enum does not
 * declare becomes `{ kind: "Unknown", name }`.
 *
 * @throws {@link DataError} for bytes that are not one valid item of `type` within `limits`.
 * @throws RangeError when `limits.maxDepth` is not a non-negative integer.
 */
export function decodeData(
  type: ContractType,
  types: Types,
  bytes: Uint8Array,
  codec: Codec,
  limits: DataLimits = DEFAULT_DATA_LIMITS,
): unknown {
  checkLimit("maxDepth", limits.maxDepth);
  try {
    if (codec === "cbor") {
      const item = decodeCborItem(bytes, limits.maxDepth, cborRefusal);
      return new CborDecoder(types).read(type, item);
    }
    const json = parseJson(bytes, limits.maxDepth, jsonRefusal);
    return new JsonDecoder(types).read(type, json);
  } catch (error) {
    if (error instanceof UnknownType) throw new DataError(codec, "unknown_type", error.message, error.path);
    throw error;
  }
}

function cborRefusal(reason: HeaderErrorReason): DataError {
  switch (reason) {
    case "too_deep":
      return new DataError("cbor", reason);
    default:
      return new DataError("cbor", "malformed", reason);
  }
}

function jsonRefusal(reason: JsonErrorReason): DataError {
  return new DataError("json", reason);
}

type Shape =
  | { readonly kind: "primitive"; readonly primitive: Primitive }
  | { readonly kind: "option"; readonly inner: ContractType; readonly unresolved: UnknownType | undefined }
  | { readonly kind: "list"; readonly inner: ContractType }
  | { readonly kind: "map"; readonly inner: ContractType }
  | { readonly kind: "tuple"; readonly items: readonly ContractType[] }
  | { readonly kind: "struct"; readonly fields: readonly Field[] }
  | { readonly kind: "enum"; readonly variants: readonly Variant[]; readonly open: boolean };

const PRIMITIVE_SHAPES = new Map<Primitive, Shape>();
const SHAPES = new WeakMap<Types, WeakMap<object, Shape>>();

function shapeOf(type: ContractType, types: Types): Shape {
  if (typeof type === "string") {
    let shape = PRIMITIVE_SHAPES.get(type);
    if (shape === undefined) {
      shape = { kind: "primitive", primitive: type };
      PRIMITIVE_SHAPES.set(type, shape);
    }
    return shape;
  }
  let cache = SHAPES.get(types);
  if (cache === undefined) {
    cache = new WeakMap();
    SHAPES.set(types, cache);
  }
  let shape = cache.get(type);
  if (shape === undefined) {
    shape = resolve(type, types);
    cache.set(type, shape);
  }
  return shape;
}

function resolve(type: ContractType, types: Types, followed: Set<string> = new Set()): Shape {
  let current: ContractType = type;
  for (;;) {
    if (typeof current === "string") return { kind: "primitive", primitive: current };
    if ("option" in current) {
      return { kind: "option", inner: current.option, unresolved: unresolvedIn(current.option, types, followed) };
    }
    if ("list" in current) return { kind: "list", inner: current.list };
    if ("map" in current) return { kind: "map", inner: current.map };
    if ("tuple" in current) return { kind: "tuple", items: current.tuple };
    if ("struct" in current) return { kind: "struct", fields: current.struct.fields };
    const named = follow(current.ref, types, followed);
    if (named instanceof UnknownType) throw named;
    if ("struct" in named) return { kind: "struct", fields: named.struct.fields };
    if ("enum" in named) return { kind: "enum", variants: named.enum.variants, open: named.enum.open === true };
    current = named.alias;
  }
}

/**
 * The type `ref` names, adding it to `followed`, or the error for a name the table lacks or one
 * already followed on the way, which is a loop that reads nothing.
 */
function follow(ref: string, types: Types, followed: Set<string>): NamedType | UnknownType {
  const named: NamedType | undefined = Object.hasOwn(types, ref) ? types[ref] : undefined;
  if (named === undefined) return new UnknownType(ref, "is not in the types table");
  if (followed.has(ref)) return new UnknownType(ref, "loops back to itself through aliases and options");
  followed.add(ref);
  return named;
}

/**
 * The error `type` raises when it reaches no type: a name the table lacks, or an alias that loops
 * back through any number of options and aliases without a value in between. It follows them with
 * the one set of names `followed`, as `pavia-codec`'s `Walk::shape` does, and only a value other
 * than null meets the error.
 */
function unresolvedIn(type: ContractType, types: Types, followed: Set<string>): UnknownType | undefined {
  let current: ContractType = type;
  for (;;) {
    if (typeof current === "string") return undefined;
    if ("option" in current) {
      current = current.option;
      continue;
    }
    if (!("ref" in current)) return undefined;
    const named = follow(current.ref, types, followed);
    if (named instanceof UnknownType) return named;
    if (!("alias" in named)) return undefined;
    current = named.alias;
  }
}

/** The type of the value an option holds when it is not null. */
function someOf(shape: Extract<Shape, { kind: "option" }>): ContractType {
  if (shape.unresolved !== undefined) throw shape.unresolved;
  return shape.inner;
}

class UnknownType extends Error {
  constructor(
    readonly ref: string,
    readonly why: string,
    readonly path = "$",
  ) {
    super(`type ${ref} ${why}`);
  }

  /** This error one `segment` further out. */
  under(segment: string): UnknownType {
    return new UnknownType(this.ref, this.why, under(this.path, segment));
  }
}

function variantNamed(variants: readonly Variant[], name: string): Variant | undefined {
  for (const variant of variants) if (variant.name === name) return variant;
  return undefined;
}

function encodeFailure(kind: EncodeErrorKind, message: string): EncodeError {
  return new EncodeError(kind, message, "$");
}

function under(path: string, segment: string): string {
  return `$${segment}${path.slice(1)}`;
}

function placed(error: EncodeError, path: string): EncodeError {
  const message = error.path === undefined ? error.message : error.message.slice(0, -` at ${error.path}`.length);
  return new EncodeError(error.kind, message, path);
}

function underEncode(error: unknown, segment: string): unknown {
  if (error instanceof UnknownType) return error.under(segment);
  return error instanceof EncodeError ? placed(error, under(error.path ?? "$", segment)) : error;
}

function underDecode(error: unknown, segment: string): unknown {
  if (error instanceof UnknownType) return error.under(segment);
  if (error instanceof DataError) return new DataError(error.codec, error.reason, error.detail, under(error.path, segment));
  return error;
}

function keySegment(key: string): string {
  return `[${JSON.stringify(key)}]`;
}

function integerOf(primitive: Primitive, value: unknown): bigint {
  const range = INTEGER_RANGE[primitive] as readonly [bigint, bigint];
  let n: bigint;
  if (primitive === "i64" || primitive === "u64") {
    if (typeof value !== "bigint") throw encodeFailure("invalid_value", `${primitive} takes a bigint`);
    n = value;
  } else {
    if (typeof value !== "number" || !Number.isInteger(value)) {
      throw encodeFailure("invalid_value", `${primitive} takes an integer number`);
    }
    n = BigInt(value);
  }
  if (n < range[0] || n > range[1]) throw encodeFailure("out_of_range", `${n} is outside ${primitive}`);
  return n;
}

function floatOf(primitive: Primitive, value: unknown): number {
  if (typeof value !== "number") throw encodeFailure("invalid_value", `${primitive} takes a number`);
  if (primitive === "f32" && !hasF32Form(value)) {
    throw encodeFailure("out_of_range", `${value} has no exact f32 form`);
  }
  return value;
}

function millisOf(value: unknown): number {
  if (!(value instanceof Date)) throw encodeFailure("invalid_value", "timestamp takes a Date");
  const millis = value.getTime();
  if (!(millis >= MIN_MILLIS && millis <= MAX_MILLIS)) {
    throw encodeFailure("out_of_range", "timestamp is outside 0000-01-01 to 9999-12-31");
  }
  return millis;
}

function uuidBytesOf(value: unknown): Uint8Array {
  const bytes = typeof value === "string" ? uuidOfText(value) : undefined;
  if (bytes === undefined) throw encodeFailure("invalid_value", "uuid takes a lowercase hyphenated string");
  return bytes;
}

function objectOf(value: unknown, what: string): Readonly<Record<string, unknown>> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw encodeFailure("invalid_value", `${what} takes an object`);
  }
  return value as Readonly<Record<string, unknown>>;
}

function arrayOf(value: unknown, what: string): readonly unknown[] {
  if (!Array.isArray(value)) throw encodeFailure("invalid_value", `${what} takes an array`);
  return value;
}

function mapOf(value: unknown): ReadonlyMap<unknown, unknown> {
  if (!(value instanceof Map)) throw encodeFailure("invalid_value", "map takes a Map");
  return value;
}

function fieldOf(object: Readonly<Record<string, unknown>>, name: string): unknown {
  return Object.hasOwn(object, name) ? object[name] : undefined;
}

function defineField(object: Record<string, unknown>, name: string, value: unknown): void {
  Object.defineProperty(object, name, { value, enumerable: true, writable: true, configurable: true });
}

function presentFields(fields: readonly Field[], object: Readonly<Record<string, unknown>>): Field[] {
  const present: Field[] = [];
  for (const field of fields) {
    const value = fieldOf(object, field.name);
    if (field.optional === true && (value === undefined || value === null)) continue;
    if (value === undefined) throw new EncodeError("missing_field", `missing field ${field.name}`, `$.${field.name}`);
    present.push(field);
  }
  return present;
}

function variantOf(variants: readonly Variant[], value: unknown): [Variant, unknown] {
  const object = objectOf(value, "an enum");
  const kind = object["kind"];
  if (typeof kind !== "string") throw encodeFailure("invalid_value", "an enum value takes a string kind");
  const variant = variantNamed(variants, kind);
  if (variant === undefined) throw encodeFailure("unknown_variant", `variant ${kind} is not in the type`);
  if (variant.type !== undefined && !("value" in object)) {
    throw encodeFailure("invalid_value", `variant ${kind} takes a value`);
  }
  if (variant.type === undefined && object["value"] !== undefined) {
    throw encodeFailure("invalid_value", `unit variant ${kind} takes no value`);
  }
  return [variant, object["value"]];
}

function checkDepth(depth: number, maxDepth: number): number {
  if (depth + 1 > maxDepth) throw encodeFailure("too_deep", `nesting is deeper than ${maxDepth}`);
  return depth + 1;
}

class CborEncoder {
  constructor(
    private readonly types: Types,
    private readonly maxDepth: number,
  ) {}

  write(out: ByteWriter, type: ContractType, value: unknown, depth: number): void {
    const shape = shapeOf(type, this.types);
    switch (shape.kind) {
      case "primitive":
        return this.primitive(out, shape.primitive, value, depth);
      case "option":
        if (value === null || value === undefined) return out.byte(0xf6);
        return this.write(out, someOf(shape), value, depth);
      case "list": {
        const items = arrayOf(value, "list");
        const inner = checkDepth(depth, this.maxDepth);
        writeHead(out, ARRAY, BigInt(items.length));
        for (let i = 0; i < items.length; i++) {
          try {
            this.write(out, shape.inner, items[i], inner);
          } catch (error) {
            throw underEncode(error, `[${i}]`);
          }
        }
        return;
      }
      case "tuple": {
        const items = arrayOf(value, "tuple");
        if (items.length !== shape.items.length) {
          throw encodeFailure("invalid_value", `tuple takes ${shape.items.length} items, not ${items.length}`);
        }
        const inner = checkDepth(depth, this.maxDepth);
        writeHead(out, ARRAY, BigInt(items.length));
        shape.items.forEach((itemType, i) => {
          try {
            this.write(out, itemType, items[i], inner);
          } catch (error) {
            throw underEncode(error, `[${i}]`);
          }
        });
        return;
      }
      case "map": {
        const map = mapOf(value);
        const inner = checkDepth(depth, this.maxDepth);
        const keys: { readonly key: string; readonly utf8: Uint8Array }[] = [];
        for (const key of map.keys()) {
          if (typeof key !== "string") throw encodeFailure("invalid_value", "map keys are strings");
          keys.push({ key, utf8: utf8Of(key) });
        }
        keys.sort((a, b) => a.utf8.length - b.utf8.length || compareBytes(a.utf8, b.utf8));
        writeHead(out, MAP, BigInt(keys.length));
        for (const { key, utf8 } of keys) {
          writeUtf8(out, utf8);
          try {
            this.write(out, shape.inner, map.get(key), inner);
          } catch (error) {
            throw underEncode(error, keySegment(key));
          }
        }
        return;
      }
      case "struct": {
        const object = objectOf(value, "a struct");
        const fields = presentFields(shape.fields, object);
        const inner = checkDepth(depth, this.maxDepth);
        writeHead(out, MAP, BigInt(fields.length));
        for (const field of fields) {
          writeText(out, field.name);
          try {
            this.write(out, field.type, fieldOf(object, field.name), inner);
          } catch (error) {
            throw underEncode(error, `.${field.name}`);
          }
        }
        return;
      }
      case "enum": {
        const [variant, payload] = variantOf(shape.variants, value);
        if (variant.type === undefined) return writeText(out, variant.name);
        const inner = checkDepth(depth, this.maxDepth);
        writeHead(out, MAP, 1n);
        writeText(out, variant.name);
        try {
          this.write(out, variant.type, payload, inner);
        } catch (error) {
          throw underEncode(error, `.${variant.name}`);
        }
        return;
      }
    }
  }

  private primitive(out: ByteWriter, primitive: Primitive, value: unknown, depth: number): void {
    switch (primitive) {
      case "unit":
        if (value !== null) throw encodeFailure("invalid_value", "unit takes null");
        return out.byte(0xf6);
      case "bool":
        if (typeof value !== "boolean") throw encodeFailure("invalid_value", "bool takes a boolean");
        return out.byte(value ? 0xf5 : 0xf4);
      case "f32":
      case "f64":
        return writeFloat(out, floatOf(primitive, value));
      case "string":
        if (typeof value !== "string") throw encodeFailure("invalid_value", "string takes a string");
        return writeText(out, value);
      case "bytes":
        if (!(value instanceof Uint8Array)) throw encodeFailure("invalid_value", "bytes takes a Uint8Array");
        writeHead(out, BYTES, BigInt(value.length));
        return out.bytes(value);
      case "timestamp": {
        const millis = millisOf(value);
        checkTagRun(1, this.maxDepth);
        writeHead(out, TAG, TIMESTAMP_TAG);
        return writeCborItem(out, epochSeconds(millis));
      }
      case "uuid": {
        const bytes = uuidBytesOf(value);
        checkTagRun(1, this.maxDepth);
        writeHead(out, TAG, UUID_TAG);
        writeHead(out, BYTES, 16n);
        return out.bytes(bytes);
      }
      case "any":
        checkItemDepth(value, depth, this.maxDepth);
        return writeCborItem(out, value as CborItem);
      default:
        return writeCborItem(out, integerItem(integerOf(primitive, value)));
    }
  }
}

function checkTagRun(run: number, maxDepth: number): void {
  if (run > maxDepth) throw encodeFailure("too_deep", `a run of ${run} tags is more than ${maxDepth}`);
}

/**
 * `value` as a {@link CborItem}, checking the fields its `kind` needs, so that an item built at run
 * time with a field missing or of the wrong type is an {@link EncodeError} instead of a `TypeError`
 * or another value on the wire. Children are checked as the walk reaches them.
 */
function itemOf(value: unknown): CborItem {
  if (typeof value !== "object" || value === null || !itemHasShape(value as Record<string, unknown>)) {
    throw encodeFailure("invalid_value", "any takes a CborItem");
  }
  return value as CborItem;
}

function itemHasShape(item: Readonly<Record<string, unknown>>): boolean {
  const field = item["value"];
  switch (item["kind"]) {
    case "uint":
    case "nint":
      return typeof field === "bigint";
    case "bytes":
      return field instanceof Uint8Array;
    case "text":
      return typeof field === "string";
    case "bool":
      return typeof field === "boolean";
    case "simple":
    case "float":
      return typeof field === "number";
    case "null":
    case "undefined":
      return true;
    case "array":
      return Array.isArray(item["items"]);
    case "map":
      return Array.isArray(item["entries"]) && item["entries"].every((entry) => Array.isArray(entry) && entry.length === 2);
    case "tag":
      return typeof item["tag"] === "bigint" && item["item"] !== undefined;
    default:
      return false;
  }
}

function checkItemDepth(value: unknown, depth: number, maxDepth: number, run = 0): void {
  const item = itemOf(value);
  switch (item.kind) {
    case "array": {
      const inner = checkDepth(depth, maxDepth);
      for (const element of item.items) checkItemDepth(element, inner, maxDepth);
      return;
    }
    case "map": {
      const inner = checkDepth(depth, maxDepth);
      for (const [key, value] of item.entries) {
        checkItemDepth(key, inner, maxDepth);
        checkItemDepth(value, inner, maxDepth);
      }
      return;
    }
    case "tag":
      checkTagRun(run + 1, maxDepth);
      return checkItemDepth(item.item, depth, maxDepth, run + 1);
    default:
      return;
  }
}

class JsonText {
  text = "";
}

class JsonEncoder {
  constructor(
    private readonly types: Types,
    private readonly maxDepth: number,
  ) {}

  write(out: JsonText, type: ContractType, value: unknown, depth: number): void {
    const shape = shapeOf(type, this.types);
    switch (shape.kind) {
      case "primitive":
        return this.primitive(out, shape.primitive, value, depth);
      case "option":
        if (value === null || value === undefined) {
          out.text += "null";
          return;
        }
        return this.write(out, someOf(shape), value, depth);
      case "list":
      case "tuple": {
        const items = arrayOf(value, shape.kind);
        if (shape.kind === "tuple" && items.length !== shape.items.length) {
          throw encodeFailure("invalid_value", `tuple takes ${shape.items.length} items, not ${items.length}`);
        }
        const inner = checkDepth(depth, this.maxDepth);
        out.text += "[";
        for (let i = 0; i < items.length; i++) {
          if (i > 0) out.text += ",";
          const itemType = shape.kind === "list" ? shape.inner : (shape.items[i] as ContractType);
          try {
            this.write(out, itemType, items[i], inner);
          } catch (error) {
            throw underEncode(error, `[${i}]`);
          }
        }
        out.text += "]";
        return;
      }
      case "map": {
        const map = mapOf(value);
        const inner = checkDepth(depth, this.maxDepth);
        const keys: string[] = [];
        for (const key of map.keys()) {
          if (typeof key !== "string") throw encodeFailure("invalid_value", "map keys are strings");
          keys.push(key);
        }
        keys.sort(compareCodePoints);
        out.text += "{";
        keys.forEach((key, i) => {
          if (i > 0) out.text += ",";
          out.text += `${jsonString(key)}:`;
          try {
            this.write(out, shape.inner, map.get(key), inner);
          } catch (error) {
            throw underEncode(error, keySegment(key));
          }
        });
        out.text += "}";
        return;
      }
      case "struct": {
        const object = objectOf(value, "a struct");
        const fields = presentFields(shape.fields, object);
        const inner = checkDepth(depth, this.maxDepth);
        out.text += "{";
        fields.forEach((field, i) => {
          if (i > 0) out.text += ",";
          out.text += `${jsonString(field.name)}:`;
          try {
            this.write(out, field.type, fieldOf(object, field.name), inner);
          } catch (error) {
            throw underEncode(error, `.${field.name}`);
          }
        });
        out.text += "}";
        return;
      }
      case "enum": {
        const [variant, payload] = variantOf(shape.variants, value);
        if (variant.type === undefined) {
          out.text += jsonString(variant.name);
          return;
        }
        const inner = checkDepth(depth, this.maxDepth);
        out.text += `{${jsonString(variant.name)}:`;
        try {
          this.write(out, variant.type, payload, inner);
        } catch (error) {
          throw underEncode(error, `.${variant.name}`);
        }
        out.text += "}";
        return;
      }
    }
  }

  private primitive(out: JsonText, primitive: Primitive, value: unknown, depth: number): void {
    switch (primitive) {
      case "unit":
        if (value !== null) throw encodeFailure("invalid_value", "unit takes null");
        out.text += "null";
        return;
      case "bool":
        if (typeof value !== "boolean") throw encodeFailure("invalid_value", "bool takes a boolean");
        out.text += value ? "true" : "false";
        return;
      case "i64":
      case "u64":
        out.text += `"${integerOf(primitive, value)}"`;
        return;
      case "f32":
      case "f64":
        out.text += jsonNumber(floatOf(primitive, value));
        return;
      case "string":
        if (typeof value !== "string") throw encodeFailure("invalid_value", "string takes a string");
        out.text += jsonString(value);
        return;
      case "bytes":
        if (!(value instanceof Uint8Array)) throw encodeFailure("invalid_value", "bytes takes a Uint8Array");
        out.text += `"${base64url(value)}"`;
        return;
      case "timestamp":
        out.text += `"${rfc3339(millisOf(value))}"`;
        return;
      case "uuid":
        uuidBytesOf(value);
        out.text += `"${value as string}"`;
        return;
      case "any":
        writeJsonItem(out, value, depth, this.maxDepth);
        return;
      default:
        out.text += String(integerOf(primitive, value));
        return;
    }
  }
}

function writeJsonItem(out: JsonText, value: unknown, depth: number, maxDepth: number): void {
  const item = itemOf(value);
  switch (item.kind) {
    case "null":
      out.text += "null";
      return;
    case "bool":
      out.text += item.value ? "true" : "false";
      return;
    case "uint":
    case "nint":
      out.text += String(integerOfItem(item));
      return;
    case "float":
      out.text += jsonNumber(item.value);
      return;
    case "text":
      out.text += jsonString(item.value);
      return;
    case "array": {
      const inner = checkDepth(depth, maxDepth);
      out.text += "[";
      item.items.forEach((element, i) => {
        if (i > 0) out.text += ",";
        writeJsonItem(out, element, inner, maxDepth);
      });
      out.text += "]";
      return;
    }
    case "map": {
      const inner = checkDepth(depth, maxDepth);
      const keys = new Set<string>();
      out.text += "{";
      item.entries.forEach(([entryKey, value], i) => {
        const key = itemOf(entryKey);
        if (key.kind !== "text") throw encodeFailure("not_json", "a JSON object key is text");
        if (keys.has(key.value)) throw encodeFailure("not_json", `duplicate key ${key.value}`);
        keys.add(key.value);
        if (i > 0) out.text += ",";
        out.text += `${jsonString(key.value)}:`;
        writeJsonItem(out, value, inner, maxDepth);
      });
      out.text += "}";
      return;
    }
    default:
      throw encodeFailure("not_json", `a ${item.kind} item has no JSON form`);
  }
}

function decodeFailure(codec: Codec, reason: DataErrorReason, detail?: string): DataError {
  return new DataError(codec, reason, detail);
}

function checkedInteger(codec: Codec, primitive: Primitive, n: bigint): number | bigint {
  const range = INTEGER_RANGE[primitive] as readonly [bigint, bigint];
  if (n < range[0] || n > range[1]) throw decodeFailure(codec, "out_of_range", `${n} is outside ${primitive}`);
  return primitive === "i64" || primitive === "u64" ? n : Number(n);
}

function checkedFloat(codec: Codec, primitive: Primitive, x: number): number {
  if (primitive !== "f32") return x;
  const single = Math.fround(x);
  if (Number.isFinite(x) && !Number.isFinite(single)) throw decodeFailure(codec, "out_of_range", `${x} is past f32`);
  return single;
}

class CborDecoder {
  constructor(private readonly types: Types) {}

  read(type: ContractType, item: CborItem): unknown {
    const shape = shapeOf(type, this.types);
    if (item.kind === "tag" && shape.kind !== "option" && !(shape.kind === "primitive" && isTagged(shape.primitive))) {
      throw decodeFailure("cbor", "undeclared_tag", `tag ${item.tag}`);
    }
    switch (shape.kind) {
      case "primitive":
        return this.primitive(shape.primitive, item);
      case "option":
        return item.kind === "null" ? null : this.read(someOf(shape), item);
      case "list":
        if (item.kind !== "array") throw decodeFailure("cbor", "wrong_type", "list takes an array");
        return item.items.map((element, i) => {
          try {
            return this.read(shape.inner, element);
          } catch (error) {
            throw underDecode(error, `[${i}]`);
          }
        });
      case "tuple":
        if (item.kind !== "array") throw decodeFailure("cbor", "wrong_type", "tuple takes an array");
        if (item.items.length !== shape.items.length) {
          throw decodeFailure("cbor", "length", `tuple takes ${shape.items.length} items`);
        }
        return shape.items.map((itemType, i) => {
          try {
            return this.read(itemType, item.items[i] as CborItem);
          } catch (error) {
            throw underDecode(error, `[${i}]`);
          }
        });
      case "map": {
        if (item.kind !== "map") throw decodeFailure("cbor", "wrong_type", "map takes a map");
        const map = new Map<string, unknown>();
        for (const [key, value] of item.entries) {
          if (key.kind !== "text") throw decodeFailure("cbor", "wrong_type", "map keys are text");
          if (map.has(key.value)) throw decodeFailure("cbor", "duplicate_key", JSON.stringify(key.value));
          try {
            map.set(key.value, this.read(shape.inner, value));
          } catch (error) {
            throw underDecode(error, keySegment(key.value));
          }
        }
        return map;
      }
      case "struct": {
        if (item.kind !== "map") throw decodeFailure("cbor", "wrong_type", "a struct takes a map");
        const given = new Map<string, CborItem>();
        for (const [key, value] of item.entries) {
          if (key.kind !== "text") throw decodeFailure("cbor", "wrong_type", "struct keys are text");
          if (given.has(key.value)) throw decodeFailure("cbor", "duplicate_key", JSON.stringify(key.value));
          given.set(key.value, value);
        }
        const result: Record<string, unknown> = {};
        for (const field of shape.fields) {
          defineField(result, field.name, this.field(field, given.get(field.name)));
          given.delete(field.name);
        }
        for (const [name, value] of given) {
          try {
            refuseTags(value);
          } catch (error) {
            throw underDecode(error, NAME.test(name) ? `.${name}` : keySegment(name));
          }
        }
        return result;
      }
      case "enum": {
        if (item.kind === "text") return unitVariant("cbor", shape.variants, shape.open, item.value);
        if (item.kind !== "map" || item.entries.length !== 1 || item.entries[0]?.[0].kind !== "text") {
          throw decodeFailure("cbor", "wrong_type", "an enum takes a variant name or a one-entry map");
        }
        const [key, payload] = item.entries[0] as readonly [CborItem, CborItem];
        const name = (key as { value: string }).value;
        const variant = dataVariant("cbor", shape.variants, shape.open, name);
        try {
          if (variant !== undefined) return { kind: name, value: this.read(variant.type as ContractType, payload) };
          refuseTags(payload);
          return { kind: "Unknown", name };
        } catch (error) {
          throw underDecode(error, `.${name}`);
        }
      }
    }
  }

  private field(field: Field, value: CborItem | undefined): unknown {
    if (value === undefined) {
      if (field.optional === true) return null;
      throw new DataError("cbor", "missing_field", field.name, `$.${field.name}`);
    }
    try {
      return this.read(field.type, value);
    } catch (error) {
      throw underDecode(error, `.${field.name}`);
    }
  }

  private primitive(primitive: Primitive, item: CborItem): unknown {
    switch (primitive) {
      case "any":
        return item;
      case "unit":
        if (item.kind !== "null") throw decodeFailure("cbor", "wrong_type", "unit takes null");
        return null;
      case "bool":
        if (item.kind !== "bool") throw decodeFailure("cbor", "wrong_type", "bool takes a boolean");
        return item.value;
      case "f32":
      case "f64":
        if (item.kind !== "float") throw decodeFailure("cbor", "wrong_type", `${primitive} takes a float`);
        return checkedFloat("cbor", primitive, item.value);
      case "string":
        if (item.kind !== "text") throw decodeFailure("cbor", "wrong_type", "string takes text");
        return item.value;
      case "bytes":
        if (item.kind !== "bytes") throw decodeFailure("cbor", "wrong_type", "bytes takes a byte string");
        return item.value;
      case "timestamp": {
        refuseOtherTag(item, TIMESTAMP_TAG);
        if (item.kind !== "tag") {
          throw decodeFailure("cbor", "wrong_type", "timestamp takes tag 1");
        }
        const millis = millisOfEpochSeconds(item.item);
        if (millis === undefined) throw decodeFailure("cbor", "out_of_range", "timestamp is not a valid instant");
        return new Date(millis);
      }
      case "uuid": {
        refuseOtherTag(item, UUID_TAG);
        if (item.kind !== "tag" || item.item.kind !== "bytes") {
          throw decodeFailure("cbor", "wrong_type", "uuid takes tag 37 over a byte string");
        }
        if (item.item.value.length !== 16) throw decodeFailure("cbor", "length", "uuid takes 16 bytes");
        return uuidText(item.item.value);
      }
      default: {
        const n = integerOfItem(item);
        if (n === undefined) throw decodeFailure("cbor", "wrong_type", `${primitive} takes an integer`);
        return checkedInteger("cbor", primitive, n);
      }
    }
  }
}

function refuseOtherTag(item: CborItem, declared: bigint): void {
  if (item.kind === "tag" && item.tag !== declared) throw decodeFailure("cbor", "undeclared_tag", `tag ${item.tag}`);
}

function isTagged(primitive: Primitive): boolean {
  return primitive === "timestamp" || primitive === "uuid" || primitive === "any";
}

const NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;

/**
 * The longest variant name an open enum's catch-all keeps, in bytes (the name grammar is ASCII), so
 * a peer cannot put a frame-sized label into application logs or metrics (#70).
 */
const MAX_UNKNOWN_NAME = 128;

function unknownVariant(codec: Codec, open: boolean, name: string): void {
  if (!open || name.length > MAX_UNKNOWN_NAME || !NAME.test(name)) {
    throw decodeFailure(codec, "unknown_variant", `variant ${JSON.stringify(name)} is not in the type`);
  }
}

/**
 * Refuses any tag inside the value of a struct field the type does not declare: such a value has
 * no declared type, so it may hold none of spec 12.3's tags. The path is built only on a refusal.
 */
function refuseTags(item: CborItem): void {
  switch (item.kind) {
    case "tag":
      throw decodeFailure("cbor", "undeclared_tag", `tag ${item.tag}`);
    case "array":
      item.items.forEach((element, i) => {
        try {
          refuseTags(element);
        } catch (error) {
          throw underDecode(error, `[${i}]`);
        }
      });
      return;
    case "map":
      for (const [key, value] of item.entries) {
        refuseTags(key);
        try {
          refuseTags(value);
        } catch (error) {
          throw key.kind === "text" ? underDecode(error, keySegment(key.value)) : error;
        }
      }
      return;
    default:
      return;
  }
}

function unitVariant(codec: Codec, variants: readonly Variant[], open: boolean, name: string): EnumValue {
  const variant = variantNamed(variants, name);
  if (variant === undefined) {
    unknownVariant(codec, open, name);
    return { kind: "Unknown", name };
  }
  if (variant.type !== undefined) throw decodeFailure(codec, "unknown_variant", `variant ${name} takes a value`);
  return { kind: name };
}

function dataVariant(codec: Codec, variants: readonly Variant[], open: boolean, name: string): Variant | undefined {
  const variant = variantNamed(variants, name);
  if (variant === undefined) {
    unknownVariant(codec, open, name);
    return undefined;
  }
  if (variant.type === undefined) throw decodeFailure(codec, "unknown_variant", `unit variant ${name} with a value`);
  return variant;
}

const JSON_INTEGER = /^(?:0|-?[1-9][0-9]*)$/;

class JsonDecoder {
  constructor(private readonly types: Types) {}

  read(type: ContractType, json: JsonValue): unknown {
    const shape = shapeOf(type, this.types);
    switch (shape.kind) {
      case "primitive":
        return this.primitive(shape.primitive, json);
      case "option":
        return json.kind === "null" ? null : this.read(someOf(shape), json);
      case "list":
        if (json.kind !== "array") throw decodeFailure("json", "wrong_type", "list takes an array");
        return json.items.map((element, i) => {
          try {
            return this.read(shape.inner, element);
          } catch (error) {
            throw underDecode(error, `[${i}]`);
          }
        });
      case "tuple":
        if (json.kind !== "array") throw decodeFailure("json", "wrong_type", "tuple takes an array");
        if (json.items.length !== shape.items.length) {
          throw decodeFailure("json", "length", `tuple takes ${shape.items.length} items`);
        }
        return shape.items.map((itemType, i) => {
          try {
            return this.read(itemType, json.items[i] as JsonValue);
          } catch (error) {
            throw underDecode(error, `[${i}]`);
          }
        });
      case "map": {
        if (json.kind !== "object") throw decodeFailure("json", "wrong_type", "map takes an object");
        const map = new Map<string, unknown>();
        for (const [key, value] of json.entries) {
          try {
            map.set(key, this.read(shape.inner, value));
          } catch (error) {
            throw underDecode(error, keySegment(key));
          }
        }
        return map;
      }
      case "struct": {
        if (json.kind !== "object") throw decodeFailure("json", "wrong_type", "a struct takes an object");
        const given = new Map(json.entries);
        const result: Record<string, unknown> = {};
        for (const field of shape.fields) defineField(result, field.name, this.field(field, given.get(field.name)));
        return result;
      }
      case "enum": {
        if (json.kind === "string") return unitVariant("json", shape.variants, shape.open, json.value);
        if (json.kind !== "object" || json.entries.length !== 1) {
          throw decodeFailure("json", "wrong_type", "an enum takes a variant name or a one-key object");
        }
        const [name, payload] = json.entries[0] as readonly [string, JsonValue];
        const variant = dataVariant("json", shape.variants, shape.open, name);
        if (variant === undefined) return { kind: "Unknown", name };
        try {
          return { kind: name, value: this.read(variant.type as ContractType, payload) };
        } catch (error) {
          throw underDecode(error, `.${name}`);
        }
      }
    }
  }

  private field(field: Field, value: JsonValue | undefined): unknown {
    if (value === undefined) {
      if (field.optional === true) return null;
      throw new DataError("json", "missing_field", field.name, `$.${field.name}`);
    }
    try {
      return this.read(field.type, value);
    } catch (error) {
      throw underDecode(error, `.${field.name}`);
    }
  }

  private primitive(primitive: Primitive, json: JsonValue): unknown {
    switch (primitive) {
      case "any":
        return itemOfJson(json);
      case "unit":
        if (json.kind !== "null") throw decodeFailure("json", "wrong_type", "unit takes null");
        return null;
      case "bool":
        if (json.kind !== "bool") throw decodeFailure("json", "wrong_type", "bool takes a boolean");
        return json.value;
      case "f32":
      case "f64": {
        if (json.kind !== "number") throw decodeFailure("json", "wrong_type", `${primitive} takes a number`);
        const x = Number(json.text);
        if (!Number.isFinite(x)) throw decodeFailure("json", "out_of_range", `${json.text} is not a finite double`);
        return checkedFloat("json", primitive, x);
      }
      case "i64":
      case "u64": {
        if (json.kind !== "string") throw decodeFailure("json", "wrong_type", `${primitive} takes a decimal string`);
        const n = integerOfText(json.value);
        if (n === undefined) throw decodeFailure("json", "not_canonical", `${primitive} takes 0 or -?[1-9][0-9]*`);
        return checkedInteger("json", primitive, n);
      }
      case "string":
        return this.text(json, "string");
      case "bytes": {
        const bytes = bytesOfBase64url(this.text(json, "bytes"));
        if (bytes === undefined) throw decodeFailure("json", "not_canonical", "bytes takes unpadded base64url");
        return bytes;
      }
      case "timestamp": {
        const millis = millisOfRfc3339(this.text(json, "timestamp"));
        if (millis === undefined) {
          throw decodeFailure("json", "not_canonical", "timestamp takes RFC 3339 in UTC with milliseconds");
        }
        return new Date(millis);
      }
      case "uuid": {
        const text = this.text(json, "uuid");
        if (uuidOfText(text) === undefined) {
          throw decodeFailure("json", "not_canonical", "uuid takes a lowercase hyphenated string");
        }
        return text;
      }
      default: {
        if (json.kind !== "number") throw decodeFailure("json", "wrong_type", `${primitive} takes a number`);
        if (!JSON_INTEGER.test(json.text)) {
          throw decodeFailure("json", "wrong_type", `${primitive} takes an integer with no fraction or exponent`);
        }
        return checkedInteger("json", primitive, BigInt(json.text));
      }
    }
  }

  private text(json: JsonValue, primitive: string): string {
    if (json.kind !== "string") throw decodeFailure("json", "wrong_type", `${primitive} takes a string`);
    return json.value;
  }
}

const U64_LIMIT = MAX_U64 + 1n;

function itemOfJson(json: JsonValue): CborItem {
  switch (json.kind) {
    case "null":
      return { kind: "null" };
    case "bool":
      return { kind: "bool", value: json.value };
    case "string":
      return { kind: "text", value: json.value };
    case "number": {
      if (JSON_INTEGER.test(json.text)) {
        const n = BigInt(json.text);
        if (n >= -U64_LIMIT && n < U64_LIMIT) return integerItem(n);
      }
      const x = Number(json.text);
      if (!Number.isFinite(x)) throw decodeFailure("json", "out_of_range", `${json.text} is not a finite double`);
      return { kind: "float", value: x };
    }
    case "array":
      return { kind: "array", items: json.items.map(itemOfJson) };
    case "object":
      return { kind: "map", entries: json.entries.map(([key, value]) => [{ kind: "text", value: key }, itemOfJson(value)]) };
  }
}
