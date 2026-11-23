import { EncodeError } from "./errors.js";

const LONE_SURROGATE = /\p{Surrogate}/u;
const UTF8_ENCODER = new TextEncoder();
const UTF8_DECODER = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

/**
 * Refuses `text` when it holds a lone UTF-16 surrogate, which has no UTF-8 form and which
 * `TextEncoder` would replace with U+FFFD.
 *
 * @throws {@link EncodeError} `lone_surrogate`.
 */
export function refuseLoneSurrogate(text: string): void {
  if (LONE_SURROGATE.test(text)) {
    throw new EncodeError("lone_surrogate", "text holds a lone UTF-16 surrogate, which has no UTF-8 form");
  }
}

/**
 * The UTF-8 bytes of `text`.
 *
 * @throws {@link EncodeError} `lone_surrogate`, as {@link refuseLoneSurrogate}.
 */
export function utf8Of(text: string): Uint8Array {
  refuseLoneSurrogate(text);
  return UTF8_ENCODER.encode(text);
}

/** The text of `bytes`, or `undefined` when they are not valid UTF-8. A byte order mark is kept. */
export function textOfUtf8(bytes: Uint8Array): string | undefined {
  try {
    return UTF8_DECODER.decode(bytes);
  } catch {
    return undefined;
  }
}
