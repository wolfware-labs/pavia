/** Session close codes (spec 15.1). On WebSocket the close frame carries `4000 + code`. */
export const CloseCode = {
  NORMAL: 0,
  PROTOCOL_ERROR: 1,
  UNSUPPORTED_VERSION: 2,
  UNAUTHENTICATED: 3,
  FORBIDDEN: 4,
  IDLE_TIMEOUT: 5,
  SLOW_CONSUMER: 6,
  GOING_AWAY: 7,
  INTERNAL: 8,
  FRAME_TOO_LARGE: 9,
  LIMIT_EXCEEDED: 10,
  RESUME_FAILED: 11,
  HANDSHAKE_TIMEOUT: 12,
  CONTRACT_MISMATCH: 13,
} as const;

/** One of the {@link CloseCode} values. */
export type CloseCode = (typeof CloseCode)[keyof typeof CloseCode];

const CLOSE_CODES: ReadonlySet<number> = new Set(Object.values(CloseCode));

/**
 * Maps a received close code to a known one. A code this runtime does not know becomes
 * `INTERNAL`, so codes added by later drafts do not break it (spec 15).
 */
export function closeCodeFromWire(value: bigint | number): CloseCode {
  const n = typeof value === "bigint" ? (value <= 0xffffn ? Number(value) : -1) : value;
  return CLOSE_CODES.has(n) ? (n as CloseCode) : CloseCode.INTERNAL;
}

/** The call status a control request is answered with when one of its fields is bad (spec 15.2). */
export const INVALID_ARGUMENT = 3;

/**
 * Why a header section was refused (spec 5.4.1). The names match the `error` field of the header
 * vectors; `key_not_unsigned` is a frame header with a key other than an unsigned integer.
 */
export type HeaderErrorReason =
  | "not_a_map"
  | "trailing_bytes"
  | "malformed"
  | "indefinite_length"
  | "tag"
  | "duplicate_key"
  | "too_deep"
  | "too_large"
  | "key_not_unsigned";

/** What is wrong with a header field of a typed frame. */
export type FieldProblem = "missing" | "wrong_type" | "unexpected" | "out_of_range";

/** A header field that is missing or invalid. */
export interface FieldError {
  /** The frame type byte. */
  readonly frameType: number;
  /** The header key; for a bad entry inside a nested map, the key of that map. */
  readonly key: number;
  /** What is wrong. */
  readonly problem: FieldProblem;
  /** The frame's `rid`, when it decoded, so a control request can be answered (spec 7.9). */
  readonly rid: bigint | undefined;
}

/**
 * The kinds of {@link ProtocolError} the wire layer raises.
 *
 * - `frame_too_large`: a frame Length above `max_frame` (FRAME_TOO_LARGE).
 * - `malformed_message`: an empty WebSocket message.
 * - `text_message`: a text WebSocket message (spec 4.3).
 * - `message_too_large`: a WebSocket message above `2 x max_frame` (spec 4.3).
 * - `leftover_bytes`: bytes after the last complete WsFrame of a message (spec 5.4).
 * - `stream_id_too_large`: a stream ID above 2^53 - 1, which this runtime cannot hold exactly.
 * - `stream_ended_mid_frame`: a stream that ends (FIN) inside a frame (spec 5.3).
 * - `malformed_header`: a header section that breaks spec 5.4.1; `reason` says which rule.
 * - `heartbeat_too_large`: a PING or PONG data section over 8 bytes (spec 7.4).
 * - `unexpected_data_section`: a data section on a frame type without one.
 * - `frame_type_not_allowed`: an unknown must-understand frame type (spec 5.6).
 * - `invalid_header_field`: a missing or ill-typed field; `field` says which.
 * - `invalid_control_request_field`: the same in a control request with a `rid`, answered with
 *   REQUEST_ERROR instead of closing the session (spec 7.9).
 */
export type ProtocolErrorKind =
  | "frame_too_large"
  | "malformed_message"
  | "text_message"
  | "message_too_large"
  | "leftover_bytes"
  | "stream_id_too_large"
  | "stream_ended_mid_frame"
  | "malformed_header"
  | "heartbeat_too_large"
  | "unexpected_data_section"
  | "frame_type_not_allowed"
  | "invalid_header_field"
  | "invalid_control_request_field";

/** What the session does about a {@link ProtocolError}. */
export type Outcome =
  | { readonly action: "close"; readonly code: CloseCode }
  | { readonly action: "request_error"; readonly rid: bigint; readonly status: typeof INVALID_ARGUMENT };

/**
 * Bytes received from the peer that break the protocol. {@link ProtocolError.outcome} says how the
 * session reacts: close with a code from spec 15.1, or answer a control request with REQUEST_ERROR.
 */
export class ProtocolError extends Error {
  override readonly name = "ProtocolError";

  /** The header rule that failed, for `malformed_header`. */
  readonly reason: HeaderErrorReason | undefined;

  /** The bad field, for `invalid_header_field` and `invalid_control_request_field`. */
  readonly field: FieldError | undefined;

  constructor(
    readonly kind: ProtocolErrorKind,
    detail: { reason?: HeaderErrorReason; field?: FieldError } = {},
  ) {
    super(detail.reason ? `${kind}: ${detail.reason}` : detail.field ? `${kind}: ${describe(detail.field)}` : kind);
    this.reason = detail.reason;
    this.field = detail.field;
  }

  /**
   * A field error, classified as a control request error when the frame is a control request
   * (spec 7.9) and its `rid` decoded.
   */
  static field(field: FieldError, controlRequest: boolean): ProtocolError {
    const control = controlRequest && field.rid !== undefined;
    return new ProtocolError(control ? "invalid_control_request_field" : "invalid_header_field", { field });
  }

  /** The reaction spec 15.1 and 7.9 prescribe. */
  get outcome(): Outcome {
    if (this.kind === "frame_too_large") return { action: "close", code: CloseCode.FRAME_TOO_LARGE };
    if (this.kind === "invalid_control_request_field" && this.field?.rid !== undefined) {
      return { action: "request_error", rid: this.field.rid, status: INVALID_ARGUMENT };
    }
    return { action: "close", code: CloseCode.PROTOCOL_ERROR };
  }
}

function describe(field: FieldError): string {
  return `${field.problem} field ${field.key} in frame type 0x${field.frameType.toString(16).padStart(2, "0")}`;
}

/**
 * The kinds of {@link EncodeError}.
 *
 * - `out_of_range`: a varint value above 2^62 - 1, or not a non-negative integer, or a raw
 *   frame's type or flags outside 0 to 255.
 * - `payload_too_large`: a frame payload no Length varint holds.
 * - `frame_too_large`: a frame above the peer's `max_frame`, or a message above `2 x max_frame`.
 * - `stream_id_out_of_range`: a stream ID that is not a non-negative safe integer.
 * - `not_an_extension_type`: an extension frame with a type byte below `0x80`.
 * - `limits_below_floor`: HELLO or WELCOME `limits` with `max_frame` or `window` below 64 KiB.
 * - `invalid_field`: a typed field value the decoder would refuse, such as a metadata name
 *   outside `[a-z0-9-]`, a heartbeat over 8 bytes or a fingerprint that is not 32 bytes.
 * - `duplicate_key`: two keys of one header map with the same encoding.
 * - `reserved_simple_value`: a CBOR simple value from 24 to 31.
 * - `lone_surrogate`: text holding a lone UTF-16 surrogate, which has no UTF-8 form.
 * - `tag_in_header`: a CBOR tag in a header, where spec 5.4.1 allows none.
 * - `invalid_value`: a data value of the wrong JavaScript type for its contract type, such as a
 *   `number` for an `i64` or an uppercase `uuid`.
 * - `missing_field`: a struct value without a field that is not `optional`.
 * - `unknown_variant`: an enum value whose `kind` the type does not declare, the open-enum
 *   catch-all among them, since it is decode-only.
 * - `unknown_type`: a `ref` to a name the types table does not hold, or to an alias that loops
 *   back to itself without reaching a type.
 * - `not_json`: a value `json` cannot hold: NaN, an infinity, or an `any` item that is a byte
 *   string, a tag, `undefined`, a simple value, or a map with a key that is not text or appears twice.
 * - `too_deep`: a data value nested deeper than the receiver's depth limit.
 */
export type EncodeErrorKind =
  | "out_of_range"
  | "payload_too_large"
  | "frame_too_large"
  | "stream_id_out_of_range"
  | "not_an_extension_type"
  | "limits_below_floor"
  | "invalid_field"
  | "duplicate_key"
  | "reserved_simple_value"
  | "lone_surrogate"
  | "tag_in_header"
  | "invalid_value"
  | "missing_field"
  | "unknown_variant"
  | "unknown_type"
  | "not_json"
  | "too_deep";

/**
 * A value this side was asked to send that has no valid encoding. Nothing is sent. For a data
 * value, `path` says where in it the problem is: `$` for the value itself, `$.sent_at`, `$[2]`,
 * `$["key"]`.
 */
export class EncodeError extends Error {
  override readonly name = "EncodeError";

  constructor(
    readonly kind: EncodeErrorKind,
    message: string = kind,
    readonly path?: string,
  ) {
    super(path === undefined ? message : `${message} at ${path}`);
  }
}

/**
 * Why a data section was refused (spec 12.3).
 *
 * - `malformed`: bytes that are not one complete CBOR item or JSON text, with the CBOR rule that
 *   failed, such as `trailing_bytes`, as `detail`.
 * - `too_deep`: arrays and maps nested deeper than the depth limit.
 * - `duplicate_key`: a map or object with a key twice.
 * - `undeclared_tag`: a CBOR tag where the type declares none (spec 12.3).
 * - `wrong_type`: an item of another kind than the type takes, such as a float for a `u8`.
 * - `out_of_range`: an integer outside its type, a finite float past the `f32` range for an
 *   `f32`, a number past the doubles, or a timestamp outside what RFC 3339 writes.
 * - `not_canonical`: text in a form `json` does not write: a 64-bit integer with a sign or
 *   leading zeros, padded base64url, a timestamp without milliseconds or not in UTC, an uppercase
 *   uuid.
 * - `missing_field`: a struct without a field that is not `optional`.
 * - `unknown_variant`: a variant a closed enum does not declare, a unit variant with a value or a
 *   data variant without one.
 * - `length`: a tuple with another number of items, or a uuid that is not 16 bytes.
 * - `unknown_type`: a `ref` to a name the types table does not hold, or to an alias that loops
 *   back to itself without reaching a type.
 */
export type DataErrorReason =
  | "malformed"
  | "too_deep"
  | "duplicate_key"
  | "undeclared_tag"
  | "wrong_type"
  | "out_of_range"
  | "not_canonical"
  | "missing_field"
  | "unknown_variant"
  | "length"
  | "unknown_type";

/**
 * A data section that does not decode as its contract type (spec 12.3). For a call it is answered
 * with INVALID_ARGUMENT; the session goes on. `path` says where: `$`, `$.sent_at`, `$[2]`.
 */
export class DataError extends Error {
  override readonly name = "DataError";

  constructor(
    readonly codec: "cbor" | "json",
    readonly reason: DataErrorReason,
    readonly detail?: string,
    readonly path: string = "$",
  ) {
    super(`${reason}${detail === undefined ? "" : `: ${detail}`} at ${path}`);
  }
}
