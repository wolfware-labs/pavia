/**
 * Whether `value` is a `Map`, also one made in another realm (an iframe, a `vm` context), where
 * `instanceof` against this realm's constructor is false. It asks `Map.prototype.has`, which
 * checks the value's own map storage, so an object that only sets `Symbol.toStringTag` to `"Map"`
 * is not taken for one.
 */
export function isMap(value: unknown): value is ReadonlyMap<unknown, unknown> {
  try {
    Map.prototype.has.call(value, undefined);
    return true;
  } catch {
    return false;
  }
}

/** Whether `value` is a `Date`, also one made in another realm, checked as {@link isMap} checks. */
export function isDate(value: unknown): value is Date {
  try {
    Date.prototype.getTime.call(value);
    return true;
  } catch {
    return false;
  }
}
