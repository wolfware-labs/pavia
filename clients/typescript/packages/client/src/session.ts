import { compareBytes } from "./bytes.js";
import { grantedCaps, serverFrameAllowed } from "./caps.js";
import { DEFAULT_HEADER_LIMITS } from "./cbor.js";
import type { Codec } from "./data.js";
import { CloseCode, closeCodeFromWire, EncodeError, ProtocolError } from "./errors.js";
import {
  type Auth,
  type ClientInfo,
  type Close,
  decodeFrame,
  type Frame,
  type Hello,
  type Limits,
  toRawFrame,
  type Welcome,
} from "./frames.js";
import { textOfUtf8, utf8Of } from "./utf8.js";
import { decodeMessage, encodeMessages, MAX_FRAME_FLOOR, type RawFrame } from "./wire.js";

/** The `max_frame` this runtime accepts when the options set none (spec 16). */
export const DEFAULT_MAX_FRAME = 1_048_576;

/** How long the transport and HELLO may take to reach WELCOME or REJECT, in ms (spec 7.1, 16). */
export const DEFAULT_HANDSHAKE_TIMEOUT = 10_000;

/** The `client` map sent in HELLO when the options set none. */
export const CLIENT_INFO: ClientInfo = { name: "pavia-ts", version: "0.1.0-alpha.0" };

/** What the session offers in HELLO and how it bounds what it receives. */
export interface SessionOptions {
  /** Codecs in preference order (HELLO key 16). Default `["cbor"]`. */
  readonly codecs?: readonly Codec[] | undefined;
  /** Capabilities requested (key 18). */
  readonly caps?: readonly string[] | undefined;
  /**
   * Limits on frames sent to this client (key 21). `max_frame` also bounds what this side reads,
   * default {@link DEFAULT_MAX_FRAME}; `max_frame` and `window` may not be below 64 KiB (spec 4.1).
   */
  readonly limits?: Limits | undefined;
  /** The fingerprint of the manifest the client was generated from (key 24, spec 12.5). */
  readonly contract?: Uint8Array | undefined;
  /** Credentials (key 33). */
  readonly auth?: Auth | undefined;
  /** The client library (key 34). Default {@link CLIENT_INFO}. */
  readonly client?: ClientInfo | undefined;
  /** Milliseconds from the start of the session to WELCOME. Default {@link DEFAULT_HANDSHAKE_TIMEOUT}. */
  readonly handshakeTimeout?: number | undefined;
}

/** Something that happened to the transport, at a time the driver passes with it. */
export type SessionInput =
  | { readonly kind: "open" }
  | { readonly kind: "message"; readonly data: Uint8Array | ArrayBuffer | string }
  | { readonly kind: "transport_closed"; readonly code?: number | undefined }
  | { readonly kind: "timeout" };

/** How a session ended. */
export type Ending =
  | {
      readonly kind: "closed";
      readonly code: CloseCode;
      readonly msg?: string | undefined;
      readonly by: "local" | "peer";
    }
  | {
      readonly kind: "rejected";
      readonly code: CloseCode;
      readonly msg: string;
      readonly retry?: bigint | undefined;
    }
  | { readonly kind: "transport_lost"; readonly code?: number | undefined };

/** What the driver must do, in the order {@link ClientSession.pollOutput} returns it. */
export type SessionOutput =
  | { readonly kind: "transmit"; readonly data: Uint8Array }
  | { readonly kind: "close_transport"; readonly code: number; readonly reason: string }
  | { readonly kind: "established"; readonly welcome: Welcome }
  | { readonly kind: "contract_mismatch"; readonly local: Uint8Array; readonly remote: Uint8Array }
  | { readonly kind: "frame"; readonly stream: number; readonly frame: Frame }
  | { readonly kind: "ended"; readonly ending: Ending };

type State =
  | { readonly kind: "connecting"; readonly deadline: number }
  | { readonly kind: "handshaking"; readonly deadline: number }
  | {
      readonly kind: "established";
      readonly welcome: Welcome;
      readonly granted: ReadonlySet<string>;
      readonly hb: number;
      readonly idle: number;
      readonly peerMaxFrame: number;
      pingAt: number;
      idleAt: number;
    }
  | { readonly kind: "ended" };

const CONTROL_STREAM = 0;

/** Frame types that travel on stream 0 only (spec 5.6). */
const CONTROL_ONLY: ReadonlySet<Frame["type"]> = new Set<Frame["type"]>([
  "HELLO",
  "WELCOME",
  "REJECT",
  "PING",
  "PONG",
  "GOAWAY",
  "CLOSE",
  "AUTH_REFRESH",
  "AUTH_RESULT",
  "ACK",
  "AUTH_EXPIRING",
  "REQUEST_ERROR",
  "SUBSCRIBE",
  "SUBSCRIBED",
  "UNSUBSCRIBE",
  "UNSUBSCRIBED",
  "PUBLICATION",
  "PRESENCE",
  "DATAGRAM",
  "DGRAM_BIND",
]);

/** Frame types a client never receives on stream 0: client-to-server ones and stream frames. */
const NOT_ON_CONTROL: ReadonlySet<Frame["type"]> = new Set<Frame["type"]>([
  "HELLO",
  "AUTH_REFRESH",
  "SUBSCRIBE",
  "UNSUBSCRIBE",
  "CALL",
  "ITEM",
  "END",
  "RESULT",
  "ERROR",
  "CANCEL",
  "CREDIT",
  "LANE_OPEN",
]);

const VERSION = 1n;
const WEBSOCKET_CLOSE_BASE = 4000;
const CLOSE_REASON_BYTES = 123;

/**
 * The client side of one session (spec 7) without I/O, after the driver loop of the C0.7 design
 * record: the driver feeds {@link SessionInput}s with {@link handleInput}, drains
 * {@link pollOutput} until it returns `undefined`, and arms one timer for {@link pollTimeout}.
 * Time is an argument, in milliseconds on any monotonic clock.
 *
 * ```text
 *  connecting ──open: HELLO──▶ handshaking ──WELCOME──▶ established
 *
 *  connecting, handshaking   deadline                    ▶ HANDSHAKE_TIMEOUT
 *  connecting                a message before open       ▶ PROTOCOL_ERROR, no HELLO sent
 *  handshaking               REJECT                      ▶ ended, rejected
 *  handshaking               WELCOME with another        ▶ CLOSE PROTOCOL_ERROR
 *                            version, an unoffered codec
 *                            or an unrequested capability
 *  handshaking               any frame but WELCOME,      ▶ CLOSE PROTOCOL_ERROR
 *                            REJECT or CLOSE on stream 0
 *  established               PING                        ▶ PONG with the same bytes
 *  established               hb with nothing sent        ▶ PING
 *  established               idle with nothing received  ▶ CLOSE IDLE_TIMEOUT
 *  any                       text message, bad WsFrame   ▶ CLOSE with the error's code
 *  any                       CLOSE received              ▶ ended, closed by the peer
 * ```
 *
 * Before WELCOME, frames sent stay under the 64 KiB `max_frame` floor every server honors; after
 * it, under the server's `max_frame`, read as at most 2^53 - 1 (spec 4.1). Every received message
 * is read with this side's `max_frame`. A frame on the wrong kind of stream or in the wrong
 * direction, or one that needs a capability WELCOME did not grant, closes with PROTOCOL_ERROR
 * (spec 5.6, 7.10); frames on call and lane streams go to the call
 * layer as `frame` outputs, which checks them further. Extension frames (0x80 and up) are ignored:
 * this runtime understands none of them (spec 5.6). Every ending yields one `ended` output, last.
 */
export class ClientSession {
  private state: State;
  private readonly outputs: SessionOutput[] = [];
  private urgent = 0;
  private now: number;
  private nextStream = 2;
  private readonly hello: Hello;
  private readonly helloFrame: RawFrame;
  private readonly maxFrame: number;

  /**
   * Starts a session at `now`, before its transport is open; the handshake deadline runs from here.
   *
   * @throws {@link EncodeError} when the options make a HELLO no server accepts: `limits_below_floor`
   * for `max_frame` or `window` below 64 KiB, `invalid_field` for a fingerprint that is not 32 bytes,
   * `frame_too_large` for a HELLO whose header is above 16 KiB (a long `auth` token). HELLO has no
   * data section, so such a HELLO also stays under the 64 KiB a server reads before WELCOME.
   * @throws RangeError when `handshakeTimeout` is not a positive finite number of milliseconds.
   */
  constructor(options: SessionOptions, now: number) {
    const handshakeTimeout = options.handshakeTimeout ?? DEFAULT_HANDSHAKE_TIMEOUT;
    if (!Number.isFinite(handshakeTimeout) || handshakeTimeout <= 0) {
      throw new RangeError(`handshakeTimeout ${handshakeTimeout} is not a positive number of milliseconds`);
    }
    this.now = now;
    this.hello = helloOf(options);
    this.helloFrame = toRawFrame(this.hello);
    refuseLargeHeader(this.helloFrame);
    this.maxFrame = safeNumber(options.limits?.maxFrame ?? BigInt(DEFAULT_MAX_FRAME));
    this.state = { kind: "connecting", deadline: now + handshakeTimeout };
  }

  /** The WELCOME, once the session is established. */
  get welcome(): Welcome | undefined {
    return this.state.kind === "established" ? this.state.welcome : undefined;
  }

  /** Whether the session has ended; it then ignores every input. */
  get ended(): boolean {
    return this.state.kind === "ended";
  }

  /** When the driver must feed a `timeout` input, or `undefined` when no timer runs. */
  pollTimeout(): number | undefined {
    switch (this.state.kind) {
      case "connecting":
      case "handshaking":
        return this.state.deadline;
      case "established":
        return Math.min(this.state.pingAt, this.state.idleAt);
      case "ended":
        return undefined;
    }
  }

  /** The next output, or `undefined` when there is nothing to do until the next input. */
  pollOutput(): SessionOutput | undefined {
    this.urgent = Math.max(0, this.urgent - 1);
    return this.outputs.shift();
  }

  /** Applies one input that arrived at `now`. Drain {@link pollOutput} afterwards. */
  handleInput(input: SessionInput, now: number): void {
    if (this.state.kind === "ended") return;
    this.now = now;
    const state = this.state;
    const pastHandshake = (state.kind === "connecting" || state.kind === "handshaking") && now >= state.deadline;
    if (input.kind === "transport_closed") return this.transportClosed(input.code);
    if (pastHandshake) return this.close(CloseCode.HANDSHAKE_TIMEOUT);
    switch (input.kind) {
      case "open":
        if (state.kind !== "connecting") return;
        this.state = { kind: "handshaking", deadline: state.deadline };
        return this.transmit(CONTROL_STREAM, this.hello, this.helloFrame);
      case "message":
        if (state.kind === "connecting") return this.close(CloseCode.PROTOCOL_ERROR);
        this.receive(input.data);
        return this.sendDuePing();
      case "timeout":
        if (state.kind === "established" && now >= state.idleAt) return this.close(CloseCode.IDLE_TIMEOUT);
        return this.sendDuePing();
    }
  }

  /**
   * The next client stream ID: even, from 2, never reused on this transport (spec 6.1).
   *
   * @throws {@link EncodeError} `stream_id_out_of_range` once the IDs pass 2^53 - 1.
   */
  openStream(): number {
    const stream = this.nextStream;
    if (!Number.isSafeInteger(stream)) throw new EncodeError("stream_id_out_of_range", "client stream IDs are used up");
    this.nextStream += 2;
    return stream;
  }

  /**
   * Queues `frame` on `stream` at `now`, once the session is established.
   *
   * @throws {@link EncodeError} for a frame with no valid encoding, `frame_too_large` for one above
   * the server's `max_frame` or a header above 16 KiB (spec 5.2, 5.4.1), and `invalid_value` when
   * the session is not established.
   */
  send(stream: number, frame: Frame, now: number): void {
    if (this.state.kind !== "established") throw new EncodeError("invalid_value", "the session is not established");
    const raw = toRawFrame(frame);
    refuseLargeHeader(raw);
    this.now = now;
    this.transmit(stream, frame, raw);
  }

  /**
   * Ends the session at `now` with CLOSE `code` and `msg` (spec 7.7), then closes the transport
   * with `4000 + code`. `msg` is cut to the 123 bytes a WebSocket close frame holds, in the CLOSE
   * frame as well, and left out when it has no UTF-8 form. Outputs still queued are dropped: frames
   * not yet sent, and frames received in the same message that the driver has not handed on yet.
   * A `code` that is not a {@link CloseCode} is sent as INTERNAL.
   */
  close(code: CloseCode = CloseCode.NORMAL, msg?: string, now: number = this.now): void {
    if (this.state.kind === "ended") return;
    const known = closeCodeFromWire(code);
    this.now = now;
    const open = this.state.kind !== "connecting";
    const reason = msg === undefined ? undefined : closeReason(msg);
    this.outputs.length = this.urgent;
    if (open) {
      this.transmit(CONTROL_STREAM, { type: "CLOSE", code: BigInt(known), ...(reason ? { msg: reason } : {}) });
    }
    this.closeTransport(known, reason);
    this.end({ kind: "closed", code: known, msg: reason || undefined, by: "local" });
  }

  private receive(data: Uint8Array | ArrayBuffer | string): void {
    if (this.state.kind === "established") this.state.idleAt = this.now + this.state.idle;
    let frames;
    try {
      frames = decodeMessage(data, this.maxFrame);
    } catch (error) {
      return this.fail(error);
    }
    for (const { stream, frame: raw } of frames) {
      if (this.state.kind === "ended") return;
      let frame: Frame;
      try {
        frame = decodeFrame(raw);
      } catch (error) {
        return this.fail(error);
      }
      this.handle(stream, frame);
    }
  }

  private handle(stream: number, frame: Frame): void {
    const state = this.state;
    if (state.kind === "handshaking") {
      if (stream !== CONTROL_STREAM) return this.close(CloseCode.PROTOCOL_ERROR);
      switch (frame.type) {
        case "WELCOME":
          return this.welcomed(frame);
        case "REJECT": {
          const code = closeCodeFromWire(frame.code);
          this.closeTransport(code, frame.msg);
          return this.end({ kind: "rejected", code, msg: frame.msg, retry: frame.retry });
        }
        case "CLOSE":
          return this.peerClosed(frame);
        default:
          return this.close(CloseCode.PROTOCOL_ERROR);
      }
    }
    if (state.kind !== "established") return;
    if (frame.type === "EXTENSION") return;
    const onControl = stream === CONTROL_STREAM;
    if (onControl ? NOT_ON_CONTROL.has(frame.type) : CONTROL_ONLY.has(frame.type)) {
      return this.close(CloseCode.PROTOCOL_ERROR);
    }
    if (!serverFrameAllowed(state.granted, frame)) return this.close(CloseCode.PROTOCOL_ERROR);
    if (!onControl) {
      this.outputs.push({ kind: "frame", stream, frame });
      return;
    }
    switch (frame.type) {
      case "PING":
        return this.transmit(CONTROL_STREAM, { type: "PONG", data: frame.data });
      case "PONG":
        return;
      case "CLOSE":
        return this.peerClosed(frame);
      case "WELCOME":
      case "REJECT":
        return this.close(CloseCode.PROTOCOL_ERROR);
      default:
        this.outputs.push({ kind: "frame", stream, frame });
    }
  }

  private welcomed(welcome: Welcome): void {
    const requested = new Set(["calls", ...(this.hello.caps ?? [])]);
    const usable =
      welcome.version === VERSION &&
      this.hello.codecs.includes(welcome.codec) &&
      welcome.caps.every((capability) => requested.has(capability));
    if (!usable || welcome.hb === 0n || welcome.idle === 0n) return this.close(CloseCode.PROTOCOL_ERROR);
    const hb = safeNumber(welcome.hb);
    const idle = safeNumber(welcome.idle);
    this.state = {
      kind: "established",
      welcome,
      granted: grantedCaps(welcome.caps),
      hb,
      idle,
      peerMaxFrame: safeNumber(welcome.limits.maxFrame),
      pingAt: this.now + hb,
      idleAt: this.now + idle,
    };
    const local = this.hello.contract;
    const remote = welcome.contract;
    if (local !== undefined && remote !== undefined && compareBytes(local, remote) !== 0) {
      this.outputs.push({ kind: "contract_mismatch", local, remote });
    }
    this.outputs.push({ kind: "established", welcome });
  }

  private peerClosed(close: Close): void {
    const code = closeCodeFromWire(close.code);
    this.closeTransport(code, close.msg);
    this.end({ kind: "closed", code, msg: close.msg, by: "peer" });
  }

  private transportClosed(wireCode: number | undefined): void {
    this.end({ kind: "transport_lost", code: wireCode });
  }

  private fail(error: unknown): void {
    if (!(error instanceof ProtocolError)) throw error;
    const outcome = error.outcome;
    this.close(outcome.action === "close" ? outcome.code : CloseCode.PROTOCOL_ERROR);
  }

  private sendDuePing(): void {
    if (this.state.kind === "established" && this.now >= this.state.pingAt) {
      this.transmit(CONTROL_STREAM, { type: "PING", data: new Uint8Array(0) });
    }
  }

  private transmit(stream: number, frame: Frame, raw: RawFrame = toRawFrame(frame)): void {
    const peerMaxFrame = this.state.kind === "established" ? this.state.peerMaxFrame : MAX_FRAME_FLOOR;
    const messages = encodeMessages([{ stream, frame: raw }], peerMaxFrame);
    const urgent = frame.type === "PING" || frame.type === "PONG" || frame.type === "CLOSE";
    for (const data of messages) {
      if (urgent) {
        this.outputs.splice(this.urgent, 0, { kind: "transmit", data });
        this.urgent++;
      } else {
        this.outputs.push({ kind: "transmit", data });
      }
    }
    if (this.state.kind === "established") this.state.pingAt = this.now + this.state.hb;
  }

  private closeTransport(code: CloseCode, msg: string | undefined): void {
    this.outputs.push({ kind: "close_transport", code: WEBSOCKET_CLOSE_BASE + code, reason: closeReason(msg ?? "") });
  }

  private end(ending: Ending): void {
    this.state = { kind: "ended" };
    this.outputs.push({ kind: "ended", ending });
  }
}

/**
 * The HELLO of `options`, copied, so that what WELCOME is checked against is what was sent even if
 * the caller changes its arrays or bytes after the session starts.
 */
function helloOf(options: SessionOptions): Hello {
  return {
    type: "HELLO",
    versions: [VERSION],
    codecs: [...(options.codecs ?? ["cbor"])],
    ...(options.caps === undefined ? {} : { caps: [...options.caps] }),
    ...(options.limits === undefined ? {} : { limits: { ...options.limits } }),
    ...(options.contract === undefined ? {} : { contract: options.contract.slice() }),
    ...(options.auth === undefined ? {} : { auth: { ...options.auth } }),
    client: { ...(options.client ?? CLIENT_INFO) },
  };
}

/**
 * Refuses a frame whose header is above the 16 KiB every receiver reads (spec 5.4.1).
 *
 * @throws {@link EncodeError} `frame_too_large`.
 */
function refuseLargeHeader(raw: RawFrame): void {
  if (raw.header.length > DEFAULT_HEADER_LIMITS.maxSize) {
    throw new EncodeError("frame_too_large", `header of ${raw.header.length} bytes is above 16 KiB`);
  }
}

/** `value` as a number, clamped to 2^53 - 1: a bound that large is never the one that decides. */
function safeNumber(value: bigint): number {
  return value > BigInt(Number.MAX_SAFE_INTEGER) ? Number.MAX_SAFE_INTEGER : Number(value);
}

/**
 * `msg` cut to the 123 bytes a WebSocket close frame holds, on a UTF-8 character boundary
 * (spec 7.7). A message with a lone surrogate, which has no UTF-8 form, gives no reason.
 */
export function closeReason(msg: string): string {
  let bytes: Uint8Array;
  try {
    bytes = utf8Of(msg);
  } catch {
    return "";
  }
  if (bytes.length <= CLOSE_REASON_BYTES) return msg;
  let end = CLOSE_REASON_BYTES;
  while (end > 0 && ((bytes[end] as number) & 0xc0) === 0x80) end--;
  return textOfUtf8(bytes.subarray(0, end)) ?? "";
}
