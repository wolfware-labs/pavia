import { isByte } from "./bytes.js";
import {
  DEFAULT_HEADER_LIMITS,
  decodeHeader,
  encodeHeader,
  type HeaderLimits,
  type HeaderMap,
  type HeaderValue,
  MAX_U64,
} from "./cbor.js";
import { EncodeError, type FieldProblem, ProtocolError } from "./errors.js";
import { encodeRawFrame, MAX_FRAME_FLOOR, type RawFrame } from "./wire.js";

/** Header keys (spec 5.7). */
export const Key = {
  RID: 1,
  METHOD: 2,
  META: 3,
  DEADLINE: 4,
  SHAPE: 5,
  CODE: 6,
  MSG: 7,
  LANE: 8,
  CHANNEL: 9,
  EPOCH: 10,
  OFFSET: 11,
  SEQ: 12,
  TOKEN: 13,
  VERSIONS: 14,
  VERSION: 15,
  CODECS: 16,
  CODEC: 17,
  CAPS: 18,
  SESSION: 19,
  RESUME: 20,
  LIMITS: 21,
  HB: 22,
  IDLE: 23,
  CONTRACT: 24,
  CREDIT: 25,
  RECOVERED: 26,
  PRESENCE: 27,
  KIND: 28,
  KEY: 29,
  NAME: 30,
  RETRY: 31,
  URL: 32,
  AUTH: 33,
  CLIENT: 34,
  SCHEME: 35,
  USER: 36,
  RESUME_WINDOW: 37,
  EXPIRES: 38,
  WINDOW: 40,
  SINCE: 41,
  RESUMED: 43,
  ACK: 44,
  NX: 45,
  TRANSPORTS: 46,
  MAX_CALLS: 48,
  MAX_FRAME: 49,
  MAX_SUBS: 50,
  MAX_DGRAM: 51,
  CLIENT_VERSION: 52,
  LOST: 53,
} as const;

/** The registered frame types (spec 5.6). */
export const FrameType = {
  HELLO: 0x01,
  WELCOME: 0x02,
  REJECT: 0x03,
  PING: 0x04,
  PONG: 0x05,
  GOAWAY: 0x06,
  CLOSE: 0x07,
  AUTH_REFRESH: 0x08,
  AUTH_RESULT: 0x09,
  ACK: 0x0a,
  AUTH_EXPIRING: 0x0b,
  REQUEST_ERROR: 0x0c,
  CALL: 0x10,
  ITEM: 0x11,
  END: 0x12,
  RESULT: 0x13,
  ERROR: 0x14,
  CANCEL: 0x15,
  CREDIT: 0x16,
  NOTIFY: 0x20,
  LANE_OPEN: 0x21,
  SUBSCRIBE: 0x30,
  SUBSCRIBED: 0x31,
  UNSUBSCRIBE: 0x32,
  UNSUBSCRIBED: 0x33,
  PUBLICATION: 0x34,
  PRESENCE: 0x35,
  DATAGRAM: 0x40,
  DGRAM_BIND: 0x41,
} as const;

/** The name of a registered frame type. */
export type FrameTypeName = keyof typeof FrameType;

const TYPE_NAMES: ReadonlyMap<number, FrameTypeName> = new Map(
  Object.entries(FrameType).map(([name, type]) => [type, name as FrameTypeName]),
);

const CARRIES_DATA: ReadonlySet<FrameTypeName> = new Set<FrameTypeName>([
  "PING",
  "PONG",
  "CALL",
  "ITEM",
  "RESULT",
  "ERROR",
  "NOTIFY",
  "SUBSCRIBE",
  "SUBSCRIBED",
  "PUBLICATION",
  "PRESENCE",
  "DATAGRAM",
]);

const CONTROL_REQUESTS: ReadonlySet<FrameTypeName> = new Set<FrameTypeName>([
  "AUTH_REFRESH",
  "SUBSCRIBE",
  "UNSUBSCRIBE",
]);

/** The lowest `max_frame` and `window` either peer may advertise in HELLO or WELCOME (spec 4.1). */
export const LIMITS_FLOOR = BigInt(MAX_FRAME_FLOOR);

/** The longest data section of a PING or PONG (spec 7.4). */
export const MAX_HEARTBEAT = 8;

/** A credential or resume token (key 13): bytes or text. */
export type Token = Uint8Array | string;

/** A lane-to-seq map (key 44): the highest contiguous seq received per lane (spec 7.8). */
export type AckMap = ReadonlyMap<bigint, bigint>;

/** Call metadata (key 3): names in `[a-z0-9-]` to text or bytes (spec 8.7). */
export type Meta = ReadonlyMap<string, string | Uint8Array>;

/** The `limits` map of HELLO (key 21, spec 7.1). Every entry is optional. */
export interface Limits {
  /** `max_calls` (key 48): concurrent calls the peer may open toward this side. */
  readonly maxCalls?: bigint | undefined;
  /** `max_frame` (key 49): the largest frame payload, in bytes; at least 64 KiB. */
  readonly maxFrame?: bigint | undefined;
  /** `max_subs` (key 50). */
  readonly maxSubs?: bigint | undefined;
  /** `max_dgram` (key 51), in bytes. */
  readonly maxDgram?: bigint | undefined;
  /** `window` (key 40): the initial per-stream WebSocket credit, in bytes; at least 64 KiB. */
  readonly window?: bigint | undefined;
}

/** The `limits` map of WELCOME (key 21, spec 7.2). Only `window` is optional. */
export interface ServerLimits {
  readonly maxCalls: bigint;
  readonly maxFrame: bigint;
  readonly maxSubs: bigint;
  readonly maxDgram: bigint;
  readonly window?: bigint | undefined;
}

/** The `auth` map (key 33, spec 7.1). */
export interface Auth {
  readonly scheme: string;
  readonly token: Token;
}

/** The `resume` map of HELLO (key 20, spec 7.8). */
export interface Resume {
  readonly token: Token;
  readonly session: string;
  readonly ack: AckMap;
  readonly lost?: readonly bigint[] | undefined;
}

/** The `client` map of HELLO (key 34). */
export interface ClientInfo {
  readonly name?: string | undefined;
  readonly version?: string | undefined;
}

/** The `since` map of SUBSCRIBE (key 41, spec 10.5). */
export interface Since {
  readonly epoch: string;
  readonly offset: bigint;
}

/** `seq` and a piggybacked `ack`, carried by sequenced frames (spec 7.8). */
export interface Sequencing {
  readonly seq?: bigint | undefined;
  readonly ack?: AckMap | undefined;
}

/** HELLO (spec 7.1). */
export interface Hello {
  readonly type: "HELLO";
  readonly versions: readonly bigint[];
  readonly codecs: readonly string[];
  readonly caps?: readonly string[] | undefined;
  readonly limits?: Limits | undefined;
  readonly contract?: Uint8Array | undefined;
  readonly auth?: Auth | undefined;
  readonly resume?: Resume | undefined;
  readonly client?: ClientInfo | undefined;
}

/** WELCOME (spec 7.2). */
export interface Welcome {
  readonly type: "WELCOME";
  readonly version: bigint;
  readonly codec: string;
  readonly caps: readonly string[];
  readonly session: string;
  readonly token?: Token | undefined;
  readonly limits: ServerLimits;
  readonly hb: bigint;
  readonly idle: bigint;
  readonly resumeWindow?: bigint | undefined;
  readonly contract?: Uint8Array | undefined;
  readonly user?: string | undefined;
  readonly resumed?: boolean | undefined;
  readonly ack?: AckMap | undefined;
  readonly lost?: readonly bigint[] | undefined;
  readonly transports?: readonly string[] | undefined;
}

/** REJECT (spec 7.3). */
export interface Reject {
  readonly type: "REJECT";
  readonly code: bigint;
  readonly msg: string;
  readonly retry?: bigint | undefined;
}

/** PING or PONG (spec 7.4): at most 8 opaque bytes, echoed by PONG. */
export interface Heartbeat {
  readonly type: "PING" | "PONG";
  readonly data: Uint8Array;
}

/** GOAWAY (spec 7.6). */
export interface Goaway {
  readonly type: "GOAWAY";
  readonly code: bigint;
  readonly msg: string;
  readonly url?: string | undefined;
  readonly retry?: bigint | undefined;
}

/** CLOSE (spec 7.7). */
export interface Close {
  readonly type: "CLOSE";
  readonly code: bigint;
  readonly msg?: string | undefined;
}

/** AUTH_REFRESH (spec 7.5). */
export interface AuthRefresh extends Sequencing {
  readonly type: "AUTH_REFRESH";
  readonly rid: bigint;
  readonly auth: Auth;
}

/** AUTH_RESULT (spec 7.5). */
export interface AuthResult extends Sequencing {
  readonly type: "AUTH_RESULT";
  readonly rid: bigint;
  readonly code: bigint;
  readonly expires?: bigint | undefined;
}

/** ACK (spec 7.8). */
export interface Ack {
  readonly type: "ACK";
  readonly ack: AckMap;
}

/** AUTH_EXPIRING (spec 7.5). */
export interface AuthExpiring extends Sequencing {
  readonly type: "AUTH_EXPIRING";
  readonly expires: bigint;
}

/** REQUEST_ERROR (spec 7.9). */
export interface RequestError extends Sequencing {
  readonly type: "REQUEST_ERROR";
  readonly rid: bigint;
  readonly code: bigint;
  readonly msg: string;
}

/** CALL (spec 8.2). The data section holds the arguments. */
export interface Call {
  readonly type: "CALL";
  readonly method: string;
  readonly shape: bigint;
  readonly deadline?: bigint | undefined;
  readonly meta?: Meta | undefined;
  readonly data: Uint8Array;
}

/** ITEM (spec 8.3). The data section holds one stream element. */
export interface Item {
  readonly type: "ITEM";
  readonly meta?: Meta | undefined;
  readonly data: Uint8Array;
}

/** END (spec 8.3). */
export interface End {
  readonly type: "END";
  readonly meta?: Meta | undefined;
}

/** RESULT (spec 8.4). The data section holds the return value. */
export interface CallResult {
  readonly type: "RESULT";
  readonly meta?: Meta | undefined;
  readonly data: Uint8Array;
}

/** ERROR (spec 8.4). The data section holds the typed error detail, if any. */
export interface CallError {
  readonly type: "ERROR";
  readonly code: bigint;
  readonly msg: string;
  readonly nx?: boolean | undefined;
  readonly meta?: Meta | undefined;
  readonly data: Uint8Array;
}

/** CANCEL (spec 8.5). */
export interface Cancel {
  readonly type: "CANCEL";
  readonly msg?: string | undefined;
}

/** CREDIT (spec 6.4). */
export interface Credit {
  readonly type: "CREDIT";
  readonly credit: bigint;
}

/** NOTIFY (spec 9.1). The data section holds the arguments. */
export interface Notify extends Sequencing {
  readonly type: "NOTIFY";
  readonly method: string;
  readonly meta?: Meta | undefined;
  readonly data: Uint8Array;
}

/** LANE_OPEN (spec 9.2). */
export interface LaneOpen {
  readonly type: "LANE_OPEN";
  readonly lane: bigint;
}

/** SUBSCRIBE (spec 10.2). The data section holds optional subscription parameters. */
export interface Subscribe extends Sequencing {
  readonly type: "SUBSCRIBE";
  readonly rid: bigint;
  readonly channel: string;
  readonly since?: Since | undefined;
  readonly presence?: boolean | undefined;
  readonly meta?: Meta | undefined;
  readonly data: Uint8Array;
}

/** SUBSCRIBED (spec 10.2). The data section holds the snapshot. */
export interface Subscribed extends Sequencing {
  readonly type: "SUBSCRIBED";
  readonly rid: bigint;
  readonly channel: string;
  readonly epoch: string;
  readonly offset: bigint;
  readonly recovered?: boolean | undefined;
  readonly data: Uint8Array;
}

/** UNSUBSCRIBE (spec 10.3). */
export interface Unsubscribe extends Sequencing {
  readonly type: "UNSUBSCRIBE";
  readonly rid: bigint;
  readonly channel: string;
}

/**
 * UNSUBSCRIBED (spec 10.3), in one of two forms that forbid each other's fields: the reply to an
 * UNSUBSCRIBE (`rid`), or a server notice (`code` and `msg`).
 */
export interface Unsubscribed extends Sequencing {
  readonly type: "UNSUBSCRIBED";
  readonly channel: string;
  readonly form: { readonly rid: bigint } | { readonly code: bigint; readonly msg: string };
}

/** PUBLICATION (spec 10.4). The data section holds the published value. */
export interface Publication extends Sequencing {
  readonly type: "PUBLICATION";
  readonly channel: string;
  readonly offset: bigint;
  readonly data: Uint8Array;
}

/** PRESENCE (spec 10.6). The data section holds the entries. */
export interface Presence extends Sequencing {
  readonly type: "PRESENCE";
  readonly channel: string;
  readonly kind: bigint;
  readonly data: Uint8Array;
}

/** DATAGRAM on the WebSocket control stream (spec 11.4). The data section holds the value. */
export interface Datagram {
  readonly type: "DATAGRAM";
  readonly key: bigint;
  readonly seq: bigint;
  readonly data: Uint8Array;
}

/** DGRAM_BIND (spec 11.2). */
export interface DgramBind extends Sequencing {
  readonly type: "DGRAM_BIND";
  readonly key: bigint;
  readonly name: string;
}

/** A frame with an extension type (`0x80` to `0xFF`), kept as received so the caller can drop it. */
export interface Extension {
  readonly type: "EXTENSION";
  readonly frameType: number;
  readonly header: Uint8Array;
  readonly data: Uint8Array;
}

/**
 * One typed frame: an entry of the frame type registry (spec 5.6) with the header fields of
 * spec 5.7 as members. Flags are not part of the model: receivers ignore them and encoders write 0.
 */
export type Frame =
  | Hello
  | Welcome
  | Reject
  | Heartbeat
  | Goaway
  | Close
  | AuthRefresh
  | AuthResult
  | Ack
  | AuthExpiring
  | RequestError
  | Call
  | Item
  | End
  | CallResult
  | CallError
  | Cancel
  | Credit
  | Notify
  | LaneOpen
  | Subscribe
  | Subscribed
  | Unsubscribe
  | Unsubscribed
  | Publication
  | Presence
  | Datagram
  | DgramBind
  | Extension;

/** Whether `name` is a metadata name: non-empty `[a-z0-9-]` (spec 8.7). */
export function isMetaName(name: string): boolean {
  return /^[a-z0-9-]+$/.test(name);
}

class Bad {
  constructor(
    readonly key: number,
    readonly problem: FieldProblem,
  ) {}
}

type Read<T> = (value: HeaderValue) => T | undefined;

class Fields {
  private constructor(private readonly entries: ReadonlyMap<bigint, HeaderValue>) {}

  static of(map: HeaderMap): Fields | undefined {
    const entries = new Map<bigint, HeaderValue>();
    for (const [key, value] of map) {
      if (key.kind !== "uint") return undefined;
      entries.set(key.value, value);
    }
    return new Fields(entries);
  }

  optional<T>(key: number, read: Read<T>): T | undefined {
    const value = this.entries.get(BigInt(key));
    if (value === undefined) return undefined;
    const typed = read(value);
    if (typed === undefined) throw new Bad(key, "wrong_type");
    return typed;
  }

  required<T>(key: number, read: Read<T>): T {
    const typed = this.optional(key, read);
    if (typed === undefined) throw new Bad(key, "missing");
    return typed;
  }

  maybe<T>(key: number, read: Read<T>): T | undefined {
    try {
      return this.optional(key, read);
    } catch {
      return undefined;
    }
  }
}

const uint: Read<bigint> = (v) => (v.kind === "uint" ? v.value : undefined);
const text: Read<string> = (v) => (v.kind === "text" ? v.value : undefined);
const boolean: Read<boolean> = (v) => (v.kind === "bool" ? v.value : undefined);
const token: Read<Token> = (v) => (v.kind === "bytes" || v.kind === "text" ? v.value : undefined);
const fingerprint: Read<Uint8Array> = (v) => (v.kind === "bytes" && v.value.length === 32 ? v.value : undefined);

function array<T>(read: Read<T>): Read<T[]> {
  return (v) => {
    if (v.kind !== "array") return undefined;
    const out: T[] = [];
    for (const item of v.items) {
      const typed = read(item);
      if (typed === undefined) return undefined;
      out.push(typed);
    }
    return out;
  };
}

const uintArray = array(uint);
const textArray = array(text);

const ackMap: Read<AckMap> = (v) => {
  if (v.kind !== "map") return undefined;
  const out = new Map<bigint, bigint>();
  for (const [lane, seq] of v.entries) {
    if (lane.kind !== "uint" || seq.kind !== "uint") return undefined;
    out.set(lane.value, seq.value);
  }
  return out;
};

const meta: Read<Meta> = (v) => {
  if (v.kind !== "map") return undefined;
  const out = new Map<string, string | Uint8Array>();
  for (const [name, value] of v.entries) {
    if (value.kind !== "text" && value.kind !== "bytes") return undefined;
    if (name.kind !== "text" || !isMetaName(name.value)) return undefined;
    out.set(name.value, value.value);
  }
  return out;
};

function nested<T>(read: (fields: Fields) => T): Read<T> {
  return (v) => {
    if (v.kind !== "map") return undefined;
    const fields = Fields.of(v.entries);
    if (fields === undefined) return undefined;
    try {
      return read(fields);
    } catch (error) {
      if (error instanceof Bad) return undefined;
      throw error;
    }
  };
}

const limits = nested<Limits>((f) => ({
  maxCalls: f.optional(Key.MAX_CALLS, uint),
  maxFrame: f.optional(Key.MAX_FRAME, uint),
  maxSubs: f.optional(Key.MAX_SUBS, uint),
  maxDgram: f.optional(Key.MAX_DGRAM, uint),
  window: f.optional(Key.WINDOW, uint),
}));

const serverLimits = nested<ServerLimits>((f) => ({
  maxCalls: f.required(Key.MAX_CALLS, uint),
  maxFrame: f.required(Key.MAX_FRAME, uint),
  maxSubs: f.required(Key.MAX_SUBS, uint),
  maxDgram: f.required(Key.MAX_DGRAM, uint),
  window: f.optional(Key.WINDOW, uint),
}));

const auth = nested<Auth>((f) => ({ scheme: f.required(Key.SCHEME, text), token: f.required(Key.TOKEN, token) }));

const resume = nested<Resume>((f) => ({
  token: f.required(Key.TOKEN, token),
  session: f.required(Key.SESSION, text),
  ack: f.required(Key.ACK, ackMap),
  lost: f.optional(Key.LOST, uintArray),
}));

const client = nested<ClientInfo>((f) => ({
  name: f.optional(Key.NAME, text),
  version: f.optional(Key.CLIENT_VERSION, text),
}));

const since = nested<Since>((f) => ({ epoch: f.required(Key.EPOCH, text), offset: f.required(Key.OFFSET, uint) }));

function belowFloor(value: bigint | undefined): boolean {
  return value !== undefined && value < LIMITS_FLOOR;
}

function checkFloor(value: bigint | undefined): void {
  if (belowFloor(value)) throw new Bad(Key.LIMITS, "out_of_range");
}

function sequencing(f: Fields): Sequencing {
  return { seq: f.optional(Key.SEQ, uint), ack: f.optional(Key.ACK, ackMap) };
}

/**
 * Builds a typed frame from a raw one. Unknown header keys and the flags are ignored (spec 5.2,
 * 5.4.1); an extension type (`0x80` and above) is returned as {@link Extension}.
 *
 * @throws {@link ProtocolError} `frame_type_not_allowed`, `unexpected_data_section`,
 * `heartbeat_too_large`, `malformed_header`, `invalid_header_field` or
 * `invalid_control_request_field`.
 */
export function decodeFrame(raw: RawFrame, headerLimits: HeaderLimits = DEFAULT_HEADER_LIMITS): Frame {
  const name = TYPE_NAMES.get(raw.type);
  if (name === undefined) {
    if (raw.type >= 0x80) return { type: "EXTENSION", frameType: raw.type, header: raw.header, data: raw.data };
    throw new ProtocolError("frame_type_not_allowed");
  }
  if (!CARRIES_DATA.has(name) && raw.data.length > 0) throw new ProtocolError("unexpected_data_section");
  if ((name === "PING" || name === "PONG") && raw.data.length > MAX_HEARTBEAT) {
    throw new ProtocolError("heartbeat_too_large");
  }
  const fields = Fields.of(decodeHeader(raw.header, headerLimits) ?? []);
  if (fields === undefined) throw new ProtocolError("malformed_header", { reason: "key_not_unsigned" });
  try {
    return typed(name, fields, raw.data);
  } catch (error) {
    if (!(error instanceof Bad)) throw error;
    const rid = fields.maybe(Key.RID, uint);
    const field = { frameType: raw.type, key: error.key, problem: error.problem, rid };
    throw ProtocolError.field(field, CONTROL_REQUESTS.has(name));
  }
}

function typed(name: FrameTypeName, f: Fields, data: Uint8Array): Frame {
  switch (name) {
    case "HELLO": {
      const versions = f.required(Key.VERSIONS, uintArray);
      const codecs = f.required(Key.CODECS, textArray);
      const caps = f.optional(Key.CAPS, textArray);
      const helloLimits = f.optional(Key.LIMITS, limits);
      checkFloor(helloLimits?.maxFrame);
      checkFloor(helloLimits?.window);
      return {
        type: name,
        versions,
        codecs,
        caps,
        limits: helloLimits,
        contract: f.optional(Key.CONTRACT, fingerprint),
        auth: f.optional(Key.AUTH, auth),
        resume: f.optional(Key.RESUME, resume),
        client: f.optional(Key.CLIENT, client),
      };
    }
    case "WELCOME": {
      const version = f.required(Key.VERSION, uint);
      const codec = f.required(Key.CODEC, text);
      const caps = f.required(Key.CAPS, textArray);
      const session = f.required(Key.SESSION, text);
      const tokenValue = f.optional(Key.TOKEN, token);
      const serverLimitsValue = f.required(Key.LIMITS, serverLimits);
      checkFloor(serverLimitsValue.maxFrame);
      checkFloor(serverLimitsValue.window);
      return {
        type: name,
        version,
        codec,
        caps,
        session,
        token: tokenValue,
        limits: serverLimitsValue,
        hb: f.required(Key.HB, uint),
        idle: f.required(Key.IDLE, uint),
        resumeWindow: f.optional(Key.RESUME_WINDOW, uint),
        contract: f.optional(Key.CONTRACT, fingerprint),
        user: f.optional(Key.USER, text),
        resumed: f.optional(Key.RESUMED, boolean),
        ack: f.optional(Key.ACK, ackMap),
        lost: f.optional(Key.LOST, uintArray),
        transports: f.optional(Key.TRANSPORTS, textArray),
      };
    }
    case "REJECT":
      return {
        type: name,
        code: f.required(Key.CODE, uint),
        msg: f.required(Key.MSG, text),
        retry: f.optional(Key.RETRY, uint),
      };
    case "PING":
    case "PONG":
      return { type: name, data };
    case "GOAWAY":
      return {
        type: name,
        code: f.required(Key.CODE, uint),
        msg: f.required(Key.MSG, text),
        url: f.optional(Key.URL, text),
        retry: f.optional(Key.RETRY, uint),
      };
    case "CLOSE":
      return { type: name, code: f.required(Key.CODE, uint), msg: f.optional(Key.MSG, text) };
    case "AUTH_REFRESH":
      return { type: name, rid: f.required(Key.RID, uint), auth: f.required(Key.AUTH, auth), ...sequencing(f) };
    case "AUTH_RESULT":
      return {
        type: name,
        rid: f.required(Key.RID, uint),
        code: f.required(Key.CODE, uint),
        expires: f.optional(Key.EXPIRES, uint),
        ...sequencing(f),
      };
    case "ACK":
      return { type: name, ack: f.required(Key.ACK, ackMap) };
    case "AUTH_EXPIRING":
      return { type: name, expires: f.required(Key.EXPIRES, uint), ...sequencing(f) };
    case "REQUEST_ERROR":
      return {
        type: name,
        rid: f.required(Key.RID, uint),
        code: f.required(Key.CODE, uint),
        msg: f.required(Key.MSG, text),
        ...sequencing(f),
      };
    case "CALL":
      return {
        type: name,
        method: f.required(Key.METHOD, text),
        shape: f.required(Key.SHAPE, uint),
        deadline: f.optional(Key.DEADLINE, uint),
        meta: f.optional(Key.META, meta),
        data,
      };
    case "ITEM":
      return { type: name, meta: f.optional(Key.META, meta), data };
    case "END":
      return { type: name, meta: f.optional(Key.META, meta) };
    case "RESULT":
      return { type: name, meta: f.optional(Key.META, meta), data };
    case "ERROR":
      return {
        type: name,
        code: f.required(Key.CODE, uint),
        msg: f.required(Key.MSG, text),
        nx: f.optional(Key.NX, boolean),
        meta: f.optional(Key.META, meta),
        data,
      };
    case "CANCEL":
      return { type: name, msg: f.optional(Key.MSG, text) };
    case "CREDIT":
      return { type: name, credit: f.required(Key.CREDIT, uint) };
    case "NOTIFY":
      return {
        type: name,
        method: f.required(Key.METHOD, text),
        meta: f.optional(Key.META, meta),
        ...sequencing(f),
        data,
      };
    case "LANE_OPEN":
      return { type: name, lane: f.required(Key.LANE, uint) };
    case "SUBSCRIBE":
      return {
        type: name,
        rid: f.required(Key.RID, uint),
        channel: f.required(Key.CHANNEL, text),
        since: f.optional(Key.SINCE, since),
        presence: f.optional(Key.PRESENCE, boolean),
        meta: f.optional(Key.META, meta),
        ...sequencing(f),
        data,
      };
    case "SUBSCRIBED":
      return {
        type: name,
        rid: f.required(Key.RID, uint),
        channel: f.required(Key.CHANNEL, text),
        epoch: f.required(Key.EPOCH, text),
        offset: f.required(Key.OFFSET, uint),
        recovered: f.optional(Key.RECOVERED, boolean),
        ...sequencing(f),
        data,
      };
    case "UNSUBSCRIBE":
      return { type: name, rid: f.required(Key.RID, uint), channel: f.required(Key.CHANNEL, text), ...sequencing(f) };
    case "UNSUBSCRIBED": {
      const rid = f.optional(Key.RID, uint);
      const code = f.optional(Key.CODE, uint);
      const msg = f.optional(Key.MSG, text);
      let form: Unsubscribed["form"];
      if (rid !== undefined && code === undefined && msg === undefined) form = { rid };
      else if (rid === undefined && code !== undefined && msg !== undefined) form = { code, msg };
      else if (rid === undefined && code !== undefined) throw new Bad(Key.MSG, "missing");
      else if (rid === undefined) throw new Bad(Key.CODE, "missing");
      else if (code !== undefined) throw new Bad(Key.CODE, "unexpected");
      else throw new Bad(Key.MSG, "unexpected");
      return { type: name, channel: f.required(Key.CHANNEL, text), form, ...sequencing(f) };
    }
    case "PUBLICATION":
      return {
        type: name,
        channel: f.required(Key.CHANNEL, text),
        offset: f.required(Key.OFFSET, uint),
        ...sequencing(f),
        data,
      };
    case "PRESENCE":
      return {
        type: name,
        channel: f.required(Key.CHANNEL, text),
        kind: f.required(Key.KIND, uint),
        ...sequencing(f),
        data,
      };
    case "DATAGRAM":
      return { type: name, key: f.required(Key.KEY, uint), seq: f.required(Key.SEQ, uint), data };
    case "DGRAM_BIND":
      return { type: name, key: f.required(Key.KEY, uint), name: f.required(Key.NAME, text), ...sequencing(f) };
  }
}

type Entries = [number, HeaderValue | undefined][];

function uintValue(value: bigint | undefined): HeaderValue | undefined {
  if (value === undefined) return undefined;
  if (typeof value !== "bigint" || value < 0n || value > MAX_U64) {
    throw new EncodeError("invalid_field", `${String(value)} is not an unsigned 64-bit integer`);
  }
  return { kind: "uint", value };
}

function textValue(value: string | undefined): HeaderValue | undefined {
  return value === undefined ? undefined : { kind: "text", value };
}

function boolValue(value: boolean | undefined): HeaderValue | undefined {
  return value === undefined ? undefined : { kind: "bool", value };
}

function tokenValue(value: Token | undefined): HeaderValue | undefined {
  if (value === undefined) return undefined;
  return typeof value === "string" ? { kind: "text", value } : { kind: "bytes", value };
}

function fingerprintValue(value: Uint8Array | undefined): HeaderValue | undefined {
  if (value === undefined) return undefined;
  if (value.length !== 32) throw new EncodeError("invalid_field", "contract fingerprint is not 32 bytes");
  return { kind: "bytes", value };
}

function uintArrayValue(values: readonly bigint[] | undefined): HeaderValue | undefined {
  return values === undefined ? undefined : { kind: "array", items: values.map((v) => uintValue(v) as HeaderValue) };
}

function textArrayValue(values: readonly string[] | undefined): HeaderValue | undefined {
  return values === undefined ? undefined : { kind: "array", items: values.map((v) => ({ kind: "text", value: v })) };
}

function nestedValue(entries: Entries): HeaderValue {
  return { kind: "map", entries: toMap(entries) };
}

function toMap(entries: Entries): HeaderMap {
  const out: [HeaderValue, HeaderValue][] = [];
  for (const [key, value] of entries) if (value !== undefined) out.push([{ kind: "uint", value: BigInt(key) }, value]);
  return out;
}

function ackMapValue(value: AckMap | undefined): HeaderValue | undefined {
  if (value === undefined) return undefined;
  const entries = [...value].map(([lane, seq]) => [uintValue(lane), uintValue(seq)] as [HeaderValue, HeaderValue]);
  return { kind: "map", entries };
}

function metaValue(value: Meta | undefined): HeaderValue | undefined {
  if (value === undefined) return undefined;
  const entries: [HeaderValue, HeaderValue][] = [];
  for (const [name, v] of value) {
    if (!isMetaName(name)) {
      throw new EncodeError("invalid_field", `metadata name ${JSON.stringify(name)} is outside [a-z0-9-]`);
    }
    const item: HeaderValue = typeof v === "string" ? { kind: "text", value: v } : { kind: "bytes", value: v };
    entries.push([{ kind: "text", value: name }, item]);
  }
  return { kind: "map", entries };
}

function limitsValue(value: Limits | ServerLimits | undefined): HeaderValue | undefined {
  if (value === undefined) return undefined;
  return nestedValue([
    [Key.WINDOW, uintValue(value.window)],
    [Key.MAX_CALLS, uintValue(value.maxCalls)],
    [Key.MAX_FRAME, uintValue(value.maxFrame)],
    [Key.MAX_SUBS, uintValue(value.maxSubs)],
    [Key.MAX_DGRAM, uintValue(value.maxDgram)],
  ]);
}

function authValue(value: Auth | undefined): HeaderValue | undefined {
  if (value === undefined) return undefined;
  return nestedValue([
    [Key.TOKEN, tokenValue(value.token)],
    [Key.SCHEME, textValue(value.scheme)],
  ]);
}

function sequencingEntries(frame: Sequencing): Entries {
  return [
    [Key.SEQ, uintValue(frame.seq)],
    [Key.ACK, ackMapValue(frame.ack)],
  ];
}

function headerEntries(frame: Exclude<Frame, Extension>): Entries {
  switch (frame.type) {
    case "HELLO":
      return [
        [Key.VERSIONS, uintArrayValue(frame.versions)],
        [Key.CODECS, textArrayValue(frame.codecs)],
        [Key.CAPS, textArrayValue(frame.caps)],
        [Key.LIMITS, limitsValue(frame.limits)],
        [Key.CONTRACT, fingerprintValue(frame.contract)],
        [Key.AUTH, authValue(frame.auth)],
        [
          Key.RESUME,
          frame.resume &&
            nestedValue([
              [Key.TOKEN, tokenValue(frame.resume.token)],
              [Key.SESSION, textValue(frame.resume.session)],
              [Key.ACK, ackMapValue(frame.resume.ack)],
              [Key.LOST, uintArrayValue(frame.resume.lost)],
            ]),
        ],
        [
          Key.CLIENT,
          frame.client &&
            nestedValue([
              [Key.NAME, textValue(frame.client.name)],
              [Key.CLIENT_VERSION, textValue(frame.client.version)],
            ]),
        ],
      ];
    case "WELCOME":
      return [
        [Key.VERSION, uintValue(frame.version)],
        [Key.CODEC, textValue(frame.codec)],
        [Key.CAPS, textArrayValue(frame.caps)],
        [Key.SESSION, textValue(frame.session)],
        [Key.TOKEN, tokenValue(frame.token)],
        [Key.LIMITS, limitsValue(frame.limits)],
        [Key.HB, uintValue(frame.hb)],
        [Key.IDLE, uintValue(frame.idle)],
        [Key.RESUME_WINDOW, uintValue(frame.resumeWindow)],
        [Key.CONTRACT, fingerprintValue(frame.contract)],
        [Key.USER, textValue(frame.user)],
        [Key.RESUMED, boolValue(frame.resumed)],
        [Key.ACK, ackMapValue(frame.ack)],
        [Key.LOST, uintArrayValue(frame.lost)],
        [Key.TRANSPORTS, textArrayValue(frame.transports)],
      ];
    case "REJECT":
      return [
        [Key.CODE, uintValue(frame.code)],
        [Key.MSG, textValue(frame.msg)],
        [Key.RETRY, uintValue(frame.retry)],
      ];
    case "PING":
    case "PONG":
      return [];
    case "GOAWAY":
      return [
        [Key.CODE, uintValue(frame.code)],
        [Key.MSG, textValue(frame.msg)],
        [Key.URL, textValue(frame.url)],
        [Key.RETRY, uintValue(frame.retry)],
      ];
    case "CLOSE":
      return [
        [Key.CODE, uintValue(frame.code)],
        [Key.MSG, textValue(frame.msg)],
      ];
    case "AUTH_REFRESH":
      return [[Key.RID, uintValue(frame.rid)], [Key.AUTH, authValue(frame.auth)], ...sequencingEntries(frame)];
    case "AUTH_RESULT":
      return [
        [Key.RID, uintValue(frame.rid)],
        [Key.CODE, uintValue(frame.code)],
        [Key.EXPIRES, uintValue(frame.expires)],
        ...sequencingEntries(frame),
      ];
    case "ACK":
      return [[Key.ACK, ackMapValue(frame.ack)]];
    case "AUTH_EXPIRING":
      return [[Key.EXPIRES, uintValue(frame.expires)], ...sequencingEntries(frame)];
    case "REQUEST_ERROR":
      return [
        [Key.RID, uintValue(frame.rid)],
        [Key.CODE, uintValue(frame.code)],
        [Key.MSG, textValue(frame.msg)],
        ...sequencingEntries(frame),
      ];
    case "CALL":
      return [
        [Key.METHOD, textValue(frame.method)],
        [Key.SHAPE, uintValue(frame.shape)],
        [Key.DEADLINE, uintValue(frame.deadline)],
        [Key.META, metaValue(frame.meta)],
      ];
    case "ITEM":
    case "END":
    case "RESULT":
      return [[Key.META, metaValue(frame.meta)]];
    case "ERROR":
      return [
        [Key.CODE, uintValue(frame.code)],
        [Key.MSG, textValue(frame.msg)],
        [Key.NX, boolValue(frame.nx)],
        [Key.META, metaValue(frame.meta)],
      ];
    case "CANCEL":
      return [[Key.MSG, textValue(frame.msg)]];
    case "CREDIT":
      return [[Key.CREDIT, uintValue(frame.credit)]];
    case "NOTIFY":
      return [[Key.METHOD, textValue(frame.method)], [Key.META, metaValue(frame.meta)], ...sequencingEntries(frame)];
    case "LANE_OPEN":
      return [[Key.LANE, uintValue(frame.lane)]];
    case "SUBSCRIBE":
      return [
        [Key.RID, uintValue(frame.rid)],
        [Key.CHANNEL, textValue(frame.channel)],
        [
          Key.SINCE,
          frame.since &&
            nestedValue([
              [Key.EPOCH, textValue(frame.since.epoch)],
              [Key.OFFSET, uintValue(frame.since.offset)],
            ]),
        ],
        [Key.PRESENCE, boolValue(frame.presence)],
        [Key.META, metaValue(frame.meta)],
        ...sequencingEntries(frame),
      ];
    case "SUBSCRIBED":
      return [
        [Key.RID, uintValue(frame.rid)],
        [Key.CHANNEL, textValue(frame.channel)],
        [Key.EPOCH, textValue(frame.epoch)],
        [Key.OFFSET, uintValue(frame.offset)],
        [Key.RECOVERED, boolValue(frame.recovered)],
        ...sequencingEntries(frame),
      ];
    case "UNSUBSCRIBE":
      return [[Key.RID, uintValue(frame.rid)], [Key.CHANNEL, textValue(frame.channel)], ...sequencingEntries(frame)];
    case "UNSUBSCRIBED": {
      const form: Entries =
        "rid" in frame.form
          ? [[Key.RID, uintValue(frame.form.rid)]]
          : [
              [Key.CODE, uintValue(frame.form.code)],
              [Key.MSG, textValue(frame.form.msg)],
            ];
      return [...form, [Key.CHANNEL, textValue(frame.channel)], ...sequencingEntries(frame)];
    }
    case "PUBLICATION":
      return [
        [Key.CHANNEL, textValue(frame.channel)],
        [Key.OFFSET, uintValue(frame.offset)],
        ...sequencingEntries(frame),
      ];
    case "PRESENCE":
      return [[Key.CHANNEL, textValue(frame.channel)], [Key.KIND, uintValue(frame.kind)], ...sequencingEntries(frame)];
    case "DATAGRAM":
      return [
        [Key.KEY, uintValue(frame.key)],
        [Key.SEQ, uintValue(frame.seq)],
      ];
    case "DGRAM_BIND":
      return [[Key.KEY, uintValue(frame.key)], [Key.NAME, textValue(frame.name)], ...sequencingEntries(frame)];
  }
}

function frameData(frame: Frame): Uint8Array {
  return "data" in frame ? frame.data : new Uint8Array(0);
}

function check(frame: Frame): void {
  if (frame.type === "EXTENSION") {
    if (!isByte(frame.frameType) || frame.frameType < 0x80) {
      throw new EncodeError("not_an_extension_type", `extension frame type ${frame.frameType} is outside 0x80 to 0xff`);
    }
  } else if (frame.type === "HELLO" || frame.type === "WELCOME") {
    if (belowFloor(frame.limits?.maxFrame) || belowFloor(frame.limits?.window)) {
      throw new EncodeError("limits_below_floor", "limits below the 64 KiB floor");
    }
  } else if ((frame.type === "PING" || frame.type === "PONG") && frame.data.length > MAX_HEARTBEAT) {
    throw new EncodeError("invalid_field", "heartbeat data section longer than 8 bytes");
  }
}

/**
 * The raw form of `frame`, as {@link encodeFrame} writes it: flags 0, the header in
 * deterministic encoding (spec 5.4.1) or HeaderLength 0 when it has no fields, and the data.
 *
 * @throws {@link EncodeError} `not_an_extension_type`, `limits_below_floor` or `invalid_field`
 * for a typed frame {@link decodeFrame} would refuse, and `lone_surrogate` for text with no UTF-8 form.
 * A frame object built outside these types, one missing a required field, is not checked.
 */
export function toRawFrame(frame: Frame): RawFrame {
  check(frame);
  if (frame.type === "EXTENSION") return { type: frame.frameType, flags: 0, header: frame.header, data: frame.data };
  const entries = toMap(headerEntries(frame));
  const header = entries.length === 0 ? new Uint8Array(0) : encodeHeader(entries);
  return { type: FrameType[frame.type], flags: 0, header, data: frameData(frame) };
}

/**
 * Encodes `frame` as Type, Flags (0), Length, HeaderLength, header and data (spec 5.2).
 *
 * @throws {@link EncodeError} for the reasons of {@link toRawFrame}, and `payload_too_large`.
 */
export function encodeFrame(frame: Frame): Uint8Array {
  return encodeRawFrame(toRawFrame(frame));
}
