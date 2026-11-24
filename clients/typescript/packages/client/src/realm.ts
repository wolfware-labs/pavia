const TYPED_ARRAY_PROTOTYPE: object = Object.getPrototypeOf(Uint8Array.prototype);
const TYPED_ARRAY_NAME = Object.getOwnPropertyDescriptor(TYPED_ARRAY_PROTOTYPE, Symbol.toStringTag)?.get as (
  this: unknown,
) => string | undefined;
const BYTE_LENGTH = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, "byteLength")?.get as (
  this: unknown,
) => number;

/**
 * Whether `value` is a `Uint8Array`, also one made in another realm (an iframe, a `vm` context,
 * jsdom), where `instanceof` against this realm's constructor is false. The type name is read
 * through the typed-array getter, which reports the array's own kind, so a `Uint16Array` or a
 * `DataView` that sets its own `Symbol.toStringTag` to `"Uint8Array"` is not taken for one.
 */
export function isBytes(value: unknown): value is Uint8Array {
  return TYPED_ARRAY_NAME.call(value) === "Uint8Array";
}

/**
 * Whether `value` is a `Map`, also one made in another realm (an iframe, a `vm` context), where
 * `instanceof` against this realm's constructor is false. It asks `Map.prototype.has`, which
 * checks the value's own map storage, so an object that only sets `Symbol.toStringTag` to `"Map"`
 * is not taken for one.
 */
export function isMap(value: unknown): value is ReadonlyMap<unknown, unknown> {
  return accepts(() => Map.prototype.has.call(value, undefined));
}

/** Whether `value` is a `Date`, also one made in another realm, checked as {@link isMap} checks. */
export function isDate(value: unknown): value is Date {
  return accepts(() => Date.prototype.getTime.call(value));
}

/**
 * Whether `value` is an `ArrayBuffer`, also one made in another realm, checked as {@link isMap}
 * checks, through the `byteLength` getter. A `SharedArrayBuffer` is not one.
 */
export function isArrayBuffer(value: unknown): value is ArrayBuffer {
  return accepts(() => BYTE_LENGTH.call(value));
}

function accepts(probe: () => unknown): boolean {
  try {
    probe();
    return true;
  } catch {
    return false;
  }
}
