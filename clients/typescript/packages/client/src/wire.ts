import { ByteWriter, isByte } from "./bytes.js";
import { EncodeError, ProtocolError } from "./errors.js";
import { isBytes } from "./realm.js";
import { decodeVarint, MAX_VARINT, varintLength, writeVarint } from "./varint.js";

/** The lowest `max_frame` a peer may advertise, and the one assumed before WELCOME (spec 4.1). */
export const MAX_FRAME_FLOOR = 65_536;

/**
 * A frame as it is on the wire (spec 5.2), before its header is parsed. `header` and `data` are
 * views of the received bytes; an empty `header` means HeaderLength 0.
 */
export interface RawFrame {
  /** The Type byte. */
  readonly type: number;
  /** The Flags byte; reserved in v1, ignored by receivers. */
  readonly flags: number;
  /** The header section bytes. */
  readonly header: Uint8Array;
  /** The data section bytes. */
  readonly data: Uint8Array;
}

/** A frame on WebSocket with its stream ID (spec 5.4). */
export interface WsFrame {
  /** The stream ID, at most 2^53 - 1. */
  readonly stream: number;
  /** The frame. */
  readonly frame: RawFrame;
}

/** The largest WebSocket message a receiver with this `max_frame` accepts (spec 4.3). */
export function messageLimit(maxFrame: number): number {
  return maxFrame * 2;
}

/**
 * The Length field of `frame`: the HeaderLength varint, the header and the data (spec 5.2).
 *
 * @throws {@link EncodeError} `payload_too_large` when no varint holds it.
 */
export function payloadLength(frame: RawFrame): number {
  const length = varintLength(frame.header.length) + frame.header.length + frame.data.length;
  if (!Number.isSafeInteger(length) || BigInt(length) > MAX_VARINT) {
    throw new EncodeError("payload_too_large", "frame payload is above 2^62 - 1 bytes");
  }
  return length;
}

function checkByte(value: number, field: string): void {
  if (!isByte(value)) {
    throw new EncodeError("out_of_range", `frame ${field} ${value} is not a byte`);
  }
}

/**
 * Appends `frame`: Type, Flags, Length, HeaderLength, header and data.
 *
 * @throws {@link EncodeError} `out_of_range` when the type or flags is not an integer from 0 to
 * 255, and `payload_too_large` when no varint holds the payload length. Nothing is written then.
 */
export function writeRawFrame(out: ByteWriter, frame: RawFrame): void {
  checkByte(frame.type, "type");
  checkByte(frame.flags, "flags");
  const length = payloadLength(frame);
  out.byte(frame.type);
  out.byte(frame.flags);
  writeVarint(out, length);
  writeVarint(out, frame.header.length);
  out.bytes(frame.header);
  out.bytes(frame.data);
}

/**
 * Encodes one frame.
 *
 * @throws {@link EncodeError} as {@link writeRawFrame} does.
 */
export function encodeRawFrame(frame: RawFrame): Uint8Array {
  const out = new ByteWriter(frame.header.length + frame.data.length + 16);
  writeRawFrame(out, frame);
  return out.finish();
}

interface Prefix {
  readonly prefixLength: number;
  readonly frameLength: number;
}

function readPrefix(bytes: Uint8Array, offset: number, maxFrame: number): Prefix | undefined {
  if (bytes.length - offset < 2) return undefined;
  const length = decodeVarint(bytes, offset + 2, BigInt(maxFrame));
  if (length.kind === "need_more") return undefined;
  if (length.kind === "above_max") throw new ProtocolError("frame_too_large");
  const prefixLength = 2 + length.length;
  return { prefixLength, frameLength: prefixLength + Number(length.value) };
}

function splitPayload(type: number, flags: number, payload: Uint8Array): RawFrame {
  const headerLength = decodeVarint(payload);
  if (headerLength.kind !== "value" || headerLength.value > BigInt(payload.length - headerLength.length)) {
    throw new ProtocolError("malformed_header");
  }
  const start = headerLength.length;
  const end = start + Number(headerLength.value);
  return { type, flags, header: payload.subarray(start, end), data: payload.subarray(end) };
}

/**
 * Reads the frame that starts at `offset`. Returns `undefined` when `bytes` ends before it does.
 * The Length is checked against `maxFrame` before the payload is looked at.
 *
 * @throws {@link ProtocolError} `frame_too_large` or `malformed_header`.
 */
export function takeFrame(
  bytes: Uint8Array,
  offset: number,
  maxFrame: number,
): { readonly frame: RawFrame; readonly end: number } | undefined {
  const prefix = readPrefix(bytes, offset, maxFrame);
  if (prefix === undefined || bytes.length - offset < prefix.frameLength) return undefined;
  const end = offset + prefix.frameLength;
  const payload = bytes.subarray(offset + prefix.prefixLength, end);
  return { frame: splitPayload(bytes[offset] as number, bytes[offset + 1] as number, payload), end };
}

/**
 * Splits one received WebSocket message into its WsFrames (spec 4.3, 5.4). A string is a text
 * message, which the binding forbids.
 *
 * @throws {@link ProtocolError} `text_message`, `message_too_large`, `malformed_message`,
 * `leftover_bytes`, `stream_id_too_large`, `frame_too_large` or `malformed_header`.
 */
export function decodeMessage(message: Uint8Array | ArrayBuffer | string, maxFrame: number): WsFrame[] {
  if (typeof message === "string") throw new ProtocolError("text_message");
  const bytes = isBytes(message) ? message : new Uint8Array(message);
  if (bytes.length > messageLimit(maxFrame)) throw new ProtocolError("message_too_large");
  if (bytes.length === 0) throw new ProtocolError("malformed_message");
  const frames: WsFrame[] = [];
  let offset = 0;
  while (offset < bytes.length) {
    const stream = decodeVarint(bytes, offset);
    if (stream.kind !== "value") throw new ProtocolError("leftover_bytes");
    if (stream.value > BigInt(Number.MAX_SAFE_INTEGER)) throw new ProtocolError("stream_id_too_large");
    const taken = takeFrame(bytes, offset + stream.length, maxFrame);
    if (taken === undefined) throw new ProtocolError("leftover_bytes");
    frames.push({ stream: Number(stream.value), frame: taken.frame });
    offset = taken.end;
  }
  return frames;
}

/**
 * Batches frames into as few WebSocket messages as fit under `2 x max_frame` of the peer, never
 * splitting a frame (spec 4.3). Before WELCOME, pass no `peerMaxFrame`: the 64 KiB floor applies.
 *
 * @throws {@link EncodeError} `frame_too_large` for a frame above the peer's `max_frame`,
 * `stream_id_out_of_range` for a stream ID that is not a non-negative safe integer, and what
 * {@link writeRawFrame} throws.
 */
export function encodeMessages(frames: readonly WsFrame[], peerMaxFrame: number = MAX_FRAME_FLOOR): Uint8Array[] {
  const limit = messageLimit(peerMaxFrame);
  const messages: Uint8Array[] = [];
  let current = new ByteWriter();
  for (const { stream, frame } of frames) {
    if (payloadLength(frame) > peerMaxFrame) throw new EncodeError("frame_too_large", "frame above the peer's max_frame");
    if (!Number.isSafeInteger(stream) || stream < 0) {
      throw new EncodeError("stream_id_out_of_range", `stream ID ${stream} is not a non-negative safe integer`);
    }
    const length = payloadLength(frame);
    const size = varintLength(stream) + 2 + varintLength(length) + length;
    if (size > limit) throw new EncodeError("frame_too_large", "WsFrame above 2 x max_frame");
    if (current.length > 0 && current.length + size > limit) {
      messages.push(current.finish());
      current = new ByteWriter();
    }
    writeVarint(current, stream);
    writeRawFrame(current, frame);
  }
  if (current.length > 0) messages.push(current.finish());
  return messages;
}

/**
 * Splits the bytes of one stream into frames written back to back, as on a WebTransport stream
 * (spec 5.3). Each Length is checked against `max_frame` as soon as its prefix arrives, before the
 * payload is buffered.
 */
export class StreamFrameDecoder {
  private buffer = new Uint8Array(0);
  private start = 0;
  private end = 0;
  private checked = 0;

  constructor(private readonly maxFrame: number) {}

  /**
   * Adds bytes received on the stream.
   *
   * @throws {@link ProtocolError} `frame_too_large` when a frame's Length is above `max_frame`;
   * nothing past that prefix is buffered.
   */
  push(chunk: Uint8Array): void {
    let at = 0;
    while (at < chunk.length) {
      if (this.checked > this.end) {
        const length = Math.min(this.checked - this.end, chunk.length - at);
        this.append(chunk.subarray(at, at + length));
        at += length;
      } else {
        this.append(chunk.subarray(at, at + 1));
        at += 1;
        const prefix = readPrefix(this.buffer.subarray(0, this.end), this.checked, this.maxFrame);
        if (prefix !== undefined) this.checked += prefix.frameLength;
      }
    }
  }

  /**
   * The next complete frame, or `undefined` until more bytes arrive.
   *
   * @throws {@link ProtocolError} `malformed_header` when the HeaderLength does not fit the payload.
   */
  next(): RawFrame | undefined {
    const taken = takeFrame(this.buffer.subarray(0, this.end), this.start, this.maxFrame);
    if (taken === undefined) return undefined;
    this.start = taken.end;
    return taken.frame;
  }

  /**
   * Call when the stream ends (FIN). Complete frames not yet taken with {@link next} are fine.
   *
   * @throws {@link ProtocolError} `stream_ended_mid_frame` when a frame is cut.
   */
  finish(): void {
    if (this.end !== this.checked) throw new ProtocolError("stream_ended_mid_frame");
  }

  private append(bytes: Uint8Array): void {
    if (this.end + bytes.length > this.buffer.length) {
      const live = this.end - this.start;
      const grown = new Uint8Array(Math.max(live + bytes.length, live * 2, 64));
      grown.set(this.buffer.subarray(this.start, this.end));
      this.buffer = grown;
      this.checked -= this.start;
      this.end = live;
      this.start = 0;
    }
    this.buffer.set(bytes, this.end);
    this.end += bytes.length;
  }
}
