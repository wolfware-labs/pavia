/** A growable byte buffer that encoders append to. */
export class ByteWriter {
  private buffer: Uint8Array;
  private used = 0;

  constructor(capacity = 64) {
    this.buffer = new Uint8Array(capacity);
  }

  /** The number of bytes written so far. */
  get length(): number {
    return this.used;
  }

  /** Appends one byte. */
  byte(value: number): void {
    this.reserve(1);
    this.buffer[this.used++] = value;
  }

  /** Appends `bytes`. */
  bytes(bytes: Uint8Array): void {
    this.reserve(bytes.length);
    this.buffer.set(bytes, this.used);
    this.used += bytes.length;
  }

  /** A copy of the bytes written so far. */
  finish(): Uint8Array {
    return this.buffer.slice(0, this.used);
  }

  private reserve(extra: number): void {
    const needed = this.used + extra;
    if (needed <= this.buffer.length) return;
    const grown = new Uint8Array(Math.max(needed, this.buffer.length * 2));
    grown.set(this.buffer.subarray(0, this.used));
    this.buffer = grown;
  }
}

/** Whether `value` is an integer from 0 to 255, so a `Uint8Array` stores it unchanged. */
export function isByte(value: number): boolean {
  return Number.isInteger(value) && value >= 0 && value <= 0xff;
}

/**
 * Whether `value` is a `Uint8Array`, also one made in another realm (an iframe, a `vm` context,
 * jsdom), where `instanceof` against this realm's constructor is false.
 */
export function isBytes(value: unknown): value is Uint8Array {
  if (value instanceof Uint8Array) return true;
  return ArrayBuffer.isView(value) && Object.prototype.toString.call(value) === "[object Uint8Array]";
}

/** Orders byte arrays bytewise, a shorter prefix first. */
export function compareBytes(a: Uint8Array, b: Uint8Array): number {
  const n = Math.min(a.length, b.length);
  for (let i = 0; i < n; i++) {
    const d = (a[i] as number) - (b[i] as number);
    if (d !== 0) return d;
  }
  return a.length - b.length;
}

/** `bytes` as lowercase hex, two digits per byte: `0aff`. */
export function toHex(bytes: Uint8Array): string {
  let hex = "";
  for (const b of bytes) hex += b.toString(16).padStart(2, "0");
  return hex;
}
