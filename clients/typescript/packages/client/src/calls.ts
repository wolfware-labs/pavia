import { type Codec, decodeData, encodeData } from "./data.js";
import { DataError, EncodeError, StatusCode, StatusError, statusCodeFromWire } from "./errors.js";
import {
  type Call,
  type CallError,
  type CallResult,
  type Frame,
  isMetaName,
  type Meta,
  type Notify,
} from "./frames.js";
import { isBytes, isMap } from "./realm.js";
import type { ContractType, Types } from "./types.js";
import { CONTROL_STREAM } from "./wire.js";

/**
 * What the runtime needs to know about a server method to notify it: generated code holds one per
 * notification method. `params` are the parameter types in contract order and `types` the
 * manifest's named types they refer to.
 */
export interface NotifySignature {
  readonly method: string;
  readonly params: readonly ContractType[];
  readonly types: Types;
}

/**
 * What the runtime needs to know about a server method to call it: generated code holds one per
 * method. Beside the parameters, `returns` is the return type and `error` the declared error type,
 * if any.
 */
export interface CallSignature extends NotifySignature {
  readonly returns: ContractType;
  readonly error?: ContractType | undefined;
  /**
   * Whether the method is `idempotent` in the manifest (spec 12.2): a client may retry it after a
   * failure whose outcome is unknown. Nothing reads it yet (the retry policy is #83); a call is sent
   * the same either way.
   */
  readonly idempotent?: boolean | undefined;
}

/** Call metadata as the caller writes it: a map or a plain object of names to text or bytes. */
export type MetaInput = Meta | Readonly<Record<string, string | Uint8Array>>;

/** Options of one call (spec 12.4). */
export interface CallOptions {
  /** Aborting it sends CANCEL and rejects the call with CANCELLED (spec 8.5). */
  readonly signal?: AbortSignal | undefined;
  /**
   * Milliseconds from now: sent as CALL `deadline` and enforced here too, by sending CANCEL and
   * rejecting with DEADLINE_EXCEEDED (spec 8.6). A fraction is rounded up.
   */
  readonly deadline?: number | undefined;
  /** Call metadata (spec 8.7). Names are `[a-z0-9-]`. */
  readonly meta?: MetaInput | undefined;
  /** Receives the RESULT's metadata (trailers), when it has any, right before the call resolves. */
  readonly onTrailers?: ((meta: Meta) => void) | undefined;
}

/** Options of one notification (spec 9.1). */
export interface NotifyOptions {
  /** Notification metadata (spec 8.7). Names are `[a-z0-9-]`. */
  readonly meta?: MetaInput | undefined;
}

/** What {@link Calls} needs from the session it runs on. */
export interface CallHost {
  /** The codec WELCOME chose. */
  readonly codec: Codec;
  /** WELCOME `limits.max_calls`. */
  readonly maxCalls: bigint;
  /** The highest client stream ID opened so far (spec 6.1). */
  readonly lastStream: number;
  /** Whether the session has ended. */
  readonly ended: boolean;
  /** Whether WELCOME granted `resume`, so every sequenced frame needs `seq` (spec 7.8). */
  readonly sequenced: boolean;
  /** Opens the next client stream with `frame` and returns its ID; a refused frame takes none. */
  open(frame: Frame): number;
  /** Sends `frame` on `stream`: an open client stream, or 0 for a NOTIFY. */
  send(stream: number, frame: Frame): void;
  /** The monotonic clock, in milliseconds. */
  now(): number;
}

/** Where a call frame goes: handled, ignored, or a protocol violation that closes the session. */
export type Delivery = "handled" | "ignored" | "protocol_error";

const UNARY = 0n;

interface Pending {
  readonly signature: CallSignature;
  readonly resolve: (value: unknown) => void;
  readonly reject: (error: unknown) => void;
  readonly onTrailers: ((meta: Meta) => void) | undefined;
  readonly signal: AbortSignal | undefined;
  readonly onAbort: () => void;
  deadlineAt: number | undefined;
  settled: boolean;
}

/**
 * The unary calls this client has open (spec 8). A call holds its stream from CALL until the
 * callee's terminal frame, RESULT or ERROR, even after its promise settled on an abort or the
 * local deadline: after CANCEL everything but the terminal frame is ignored (spec 8.5), and the
 * stream still counts toward WELCOME `max_calls` (spec 6.5).
 */
export class Calls {
  private readonly pending = new Map<number, Pending>();

  constructor(private readonly host: CallHost) {}

  /** The earliest local deadline, or `undefined` when no call has one. */
  pollTimeout(): number | undefined {
    let at: number | undefined;
    for (const call of this.pending.values()) {
      if (call.deadlineAt !== undefined && (at === undefined || call.deadlineAt < at)) at = call.deadlineAt;
    }
    return at;
  }

  /** Cancels, with DEADLINE_EXCEEDED, every call whose local deadline has passed. */
  expire(now: number): void {
    for (const [stream, call] of this.pending) {
      if (call.deadlineAt !== undefined && now >= call.deadlineAt) {
        this.cancel(stream, call, StatusCode.DEADLINE_EXCEEDED, "the call's deadline passed");
      }
    }
  }

  /**
   * Sends NOTIFY on stream 0 for `signature.method` with `args` (spec 9.1). Nothing answers it.
   *
   * @throws {@link StatusError} with `nx`, sending nothing: UNAVAILABLE once the session has ended,
   * UNIMPLEMENTED while `resume` is granted, since NOTIFY is sequenced then and this client does not
   * number frames yet, INVALID_ARGUMENT for a metadata name outside `[a-z0-9-]`, arguments of the
   * wrong count or type, or a NOTIFY that does not encode.
   */
  notify(signature: NotifySignature, args: readonly unknown[], options: NotifyOptions = {}): void {
    if (this.host.ended) throw refusal(StatusCode.UNAVAILABLE, "the session has ended");
    if (this.host.sequenced) {
      throw refusal(StatusCode.UNIMPLEMENTED, "NOTIFY needs seq while resume is granted; this client sends none");
    }
    const meta = metaOf(options.meta);
    if (typeof meta === "string") throw refusal(StatusCode.INVALID_ARGUMENT, meta);
    const data = this.encodeArgs(signature, args);
    if (data instanceof StatusError) throw data;
    const frame: Notify = {
      type: "NOTIFY",
      method: signature.method,
      ...(meta === undefined ? {} : { meta }),
      data,
    };
    try {
      this.host.send(CONTROL_STREAM, frame);
    } catch (error) {
      if (!(error instanceof EncodeError)) throw error;
      throw refusal(StatusCode.INVALID_ARGUMENT, `the NOTIFY does not encode: ${error.message}`, error);
    }
  }

  private encodeArgs(signature: NotifySignature, args: readonly unknown[]): Uint8Array | StatusError {
    if (args.length !== signature.params.length) {
      const count = signature.params.length;
      return refusal(StatusCode.INVALID_ARGUMENT, `${signature.method} takes ${count} arguments, not ${args.length}`);
    }
    try {
      return encodeData({ tuple: signature.params }, signature.types, args, this.host.codec);
    } catch (error) {
      if (!(error instanceof EncodeError)) throw error;
      return refusal(StatusCode.INVALID_ARGUMENT, `the arguments do not encode: ${error.message}`, error);
    }
  }

  /**
   * Calls `signature.method` with `args`. The promise resolves with the decoded return value and
   * rejects with a {@link StatusError}; `nx` is true when nothing was sent. It rejects with a
   * RangeError for a `deadline` that is not a positive number of milliseconds up to 2^53 - 1.
   */
  call(signature: CallSignature, args: readonly unknown[], options: CallOptions = {}): Promise<unknown> {
    const start = this.host.now();
    return new Promise((resolve, reject) => {
      const refuse = (code: StatusCode, message: string, cause?: unknown) => reject(refusal(code, message, cause));
      if (this.host.ended) return refuse(StatusCode.UNAVAILABLE, "the session has ended");
      if (options.signal?.aborted) return refuse(StatusCode.CANCELLED, "the call was cancelled before it was sent");
      const deadline = options.deadline;
      if (deadline !== undefined && !(deadline > 0 && deadline <= Number.MAX_SAFE_INTEGER)) {
        return reject(new RangeError(`deadline ${deadline} is not a positive number of milliseconds up to 2^53 - 1`));
      }
      const meta = metaOf(options.meta);
      if (typeof meta === "string") return refuse(StatusCode.INVALID_ARGUMENT, meta);
      const data = this.encodeArgs(signature, args);
      if (data instanceof StatusError) return reject(data);
      if (BigInt(this.pending.size) >= this.host.maxCalls) {
        return refuse(StatusCode.RESOURCE_EXHAUSTED, `${this.pending.size} calls are open, the server's max_calls`);
      }
      const millis = deadline === undefined ? undefined : Math.ceil(deadline);
      const frame: Call = {
        type: "CALL",
        method: signature.method,
        shape: UNARY,
        ...(millis === undefined ? {} : { deadline: BigInt(millis) }),
        ...(meta === undefined ? {} : { meta }),
        data,
      };
      let stream: number;
      try {
        stream = this.host.open(frame);
      } catch (error) {
        if (!(error instanceof EncodeError)) throw error;
        return refuse(StatusCode.INVALID_ARGUMENT, `the CALL does not encode: ${error.message}`, error);
      }
      const signal = options.signal;
      const entry: Pending = {
        signature,
        resolve,
        reject,
        onTrailers: options.onTrailers,
        signal,
        onAbort: () => this.cancel(stream, entry, StatusCode.CANCELLED, "the call was cancelled"),
        deadlineAt: millis === undefined ? undefined : start + millis,
        settled: false,
      };
      this.pending.set(stream, entry);
      signal?.addEventListener("abort", entry.onAbort, { once: true });
      if (signal?.aborted) entry.onAbort();
    });
  }

  /**
   * Hands a frame received on client stream `stream` to its call. A frame for a stream at or below
   * the highest one opened that holds no call is ignored; one above it, a CALL or LANE_OPEN, which
   * the server never sends on a client stream, a CANCEL from the server, or an ITEM or END on a
   * unary call that was not cancelled breaks the protocol (spec 6.1, 6.3). CREDIT is ignored: a
   * unary call sends nothing after CALL.
   */
  receive(stream: number, frame: Frame): Delivery {
    if (frame.type === "CALL" || frame.type === "LANE_OPEN") return "protocol_error";
    const call = this.pending.get(stream);
    if (call === undefined) return stream <= this.host.lastStream ? "ignored" : "protocol_error";
    switch (frame.type) {
      case "RESULT":
        this.pending.delete(stream);
        this.finish(call);
        if (!call.settled) this.result(call, frame);
        return "handled";
      case "ERROR":
        this.pending.delete(stream);
        this.finish(call);
        if (!call.settled) this.error(call, frame);
        return "handled";
      case "CREDIT":
        return "ignored";
      default:
        return call.settled ? "ignored" : "protocol_error";
    }
  }

  /** Rejects every open call with UNAVAILABLE: the session ended, so their outcome is unknown. */
  end(): void {
    for (const call of this.pending.values()) {
      this.finish(call);
      if (!call.settled) this.settle(call, new StatusError(StatusCode.UNAVAILABLE, "the session ended"));
    }
    this.pending.clear();
  }

  private cancel(stream: number, call: Pending, code: StatusCode, message: string): void {
    this.finish(call);
    if (call.settled) return;
    if (!this.host.ended) this.host.send(stream, { type: "CANCEL" });
    this.settle(call, new StatusError(code, message));
  }

  private result(call: Pending, frame: CallResult): void {
    const { signature } = call;
    let value: unknown;
    try {
      value = decodeData(signature.returns, signature.types, frame.data, this.host.codec);
    } catch (error) {
      if (!(error instanceof DataError)) throw error;
      return this.settle(call, internal("the RESULT does not decode", error));
    }
    call.settled = true;
    try {
      if (frame.meta !== undefined) call.onTrailers?.(frame.meta);
    } catch (error) {
      return call.reject(error);
    }
    call.resolve(value);
  }

  private error(call: Pending, frame: CallError): void {
    const { signature } = call;
    let detail: unknown;
    if (frame.data.length !== 0) {
      if (signature.error === undefined) {
        return this.settle(call, internal("the ERROR has a detail, but the method declares no error type"));
      }
      try {
        detail = decodeData(signature.error, signature.types, frame.data, this.host.codec);
      } catch (error) {
        if (!(error instanceof DataError)) throw error;
        return this.settle(call, internal("the ERROR detail does not decode", error));
      }
    }
    const fields = { detail, nx: frame.nx ?? false, meta: frame.meta };
    this.settle(call, new StatusError(statusCodeFromWire(frame.code), frame.msg, fields));
  }

  private finish(call: Pending): void {
    call.deadlineAt = undefined;
    call.signal?.removeEventListener("abort", call.onAbort);
  }

  private settle(call: Pending, error: StatusError): void {
    call.settled = true;
    call.reject(error);
  }
}

function internal(message: string, cause?: DataError): StatusError {
  return new StatusError(StatusCode.INTERNAL, message, { cause });
}

/** A refused metadata name as message text: quoted text, or the type of anything else. */
function nameText(name: unknown): string {
  return typeof name === "string" ? JSON.stringify(name) : `of type ${typeof name}`;
}

/** A call or notification refused before anything was sent: `nx` is true. */
function refusal(code: StatusCode, message: string, cause?: unknown): StatusError {
  return new StatusError(code, message, { nx: true, cause });
}

/** `input` as a {@link Meta}, `undefined` when absent, or why it is not metadata. */
function metaOf(input: MetaInput | undefined): Meta | undefined | string {
  if (input === undefined) return undefined;
  const entries = isMap(input) ? [...input.entries()] : Object.entries(input);
  for (const [name, value] of entries) {
    if (!isMetaName(name)) return `metadata name ${nameText(name)} is not [a-z0-9-] text`;
    if (typeof value !== "string" && !isBytes(value)) {
      return `metadata ${JSON.stringify(name)} is neither text nor bytes`;
    }
  }
  return new Map(entries);
}
