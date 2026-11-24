import { decode, encode, same } from "@pavia/vectors/cbor";
import { type Diag, hexToBytes, parse, patternCount, refs } from "@pavia/vectors/diag";
import { vi } from "vitest";
import {
  type CallSignature,
  CloseCode,
  type Codec,
  type Connection,
  type ConnectOptions,
  connect,
  DEFAULT_MAX_FRAME,
  decodeMessage,
  encodeMessages,
  FrameType,
  Key,
  type RawFrame,
  SUBPROTOCOL,
  type WebSocketLike,
  type WsFrame,
} from "../src/index.js";
import { arrayBufferOf, files, type Json, value } from "./support.js";

const { HELLO, PING, PONG, CLOSE, CALL, CANCEL } = FrameType;
const HEARTBEATS = new Set<number>([PING, PONG]);
const SEND_WITHIN = 5000;
const ABNORMAL_CLOSURE = 1006;
const ENDED = "the transport is already closed";

/** One case of a client script, `vectors/scripts/*.json` with `"runner": "client"`. */
export interface ClientCase {
  readonly id: string;
  readonly c: Json;
}

/** Every client script case, sorted by file. */
export function clientCases(): ClientCase[] {
  return files("scripts")
    .filter(({ doc }) => doc["runner"] === "client")
    .flatMap(({ file, doc }) =>
      (doc["cases"] as Json[]).map((c) => ({ id: `scripts/${file}#${String(c["id"])}`, c })),
    );
}

/** How a call a script made settled: its value and trailers, or its error. */
export type CallOutcome =
  | { readonly ok: true; readonly value: unknown; readonly trailers: unknown }
  | { readonly ok: false; readonly error: unknown };

/** What a script run leaves for checks the wire cannot express. */
export interface Run {
  readonly connection: Connection | undefined;
  readonly warnings: readonly string[];
  /** Each call a `send` of CALL made, by its stream, settled or still pending. */
  readonly calls: ReadonlyMap<number, Promise<CallOutcome>>;
}

interface Started {
  readonly outcome: Promise<CallOutcome>;
  readonly abort: AbortController;
  readonly deadline: boolean;
}

interface Script {
  readonly server: ScriptServer;
  readonly connection: () => Connection | undefined;
  readonly calls: Map<number, Started>;
}

/** Hooks a test passes to {@link runClientScript}. */
export interface RunHooks {
  /** Called as soon as `connect()` resolves, before events held from WELCOME are dispatched. */
  readonly onConnection?: (connection: Connection) => void;
}

type Data = { kind: "empty" } | { kind: "cbor"; diag: Diag } | { kind: "exact"; bytes: Uint8Array };

interface FrameSpec {
  readonly stream: number;
  readonly type: number;
  readonly flags: number;
  readonly header: Diag | undefined;
  readonly data: Data;
}

type Step =
  | { verb: "send"; frame: FrameSpec; repeat: number }
  | { verb: "expect"; frame: FrameSpec }
  | { verb: "expect_nothing_within"; ms: number }
  | { verb: "wait"; ms: number }
  | { verb: "close_transport" }
  | { verb: "expect_close"; code: number; within: number };

/**
 * Plays the server's side of a client script against {@link connect} over an in-memory
 * WebSocket (`vectors/README.md`, "Client scripts"). Time is vitest's fake clock, which the
 * caller turns on. Throws naming the first step that fails.
 */
export async function runClientScript(c: Json, hooks: RunHooks = {}): Promise<Run> {
  const id = String(c["id"]);
  const codec = String(c["codec"]);
  const steps = (c["steps"] as Json[]).map((step, index) => parseStep(step, codec, `${id}: step ${index + 1}`));
  const hello = steps[0];
  if (hello?.verb !== "send" || hello.frame.type !== HELLO) {
    throw new Error(`${id}: the first step is not a send of HELLO`);
  }
  const server = new ScriptServer();
  const warnings: string[] = [];
  let connection: Connection | undefined;
  let failure: Error | undefined;
  const options: ConnectOptions = {
    ...helloOptions(hello.frame.header),
    WebSocket: server.constructorFor(),
    now: () => Date.now(),
    warn: (message) => warnings.push(message),
  };
  connect("ws://127.0.0.1/rt", options).then(
    (established) => {
      connection = established;
      hooks.onConnection?.(established);
    },
    (error: unknown) => {
      failure = error instanceof Error ? error : new Error(String(error));
    },
  );
  server.socket().open(SUBPROTOCOL);
  await settle();
  let explained = false;
  const script: Script = { server, connection: () => connection, calls: new Map() };
  for (const [index, step] of steps.entries()) {
    const where = `${id}: step ${index + 1} (${step.verb})`;
    const problem = await play(step, script);
    if (problem !== undefined) throw new Error(`${where}: ${problem}`);
    if (step.verb === "expect_close" || step.verb === "close_transport") explained = true;
  }
  if (failure !== undefined && !explained) {
    throw new Error(`${id}: connect() failed and no step expects it: ${failure.message}`);
  }
  const calls = new Map([...script.calls].map(([stream, started]) => [stream, started.outcome]));
  return { connection, warnings, calls };
}

type Problem = string | undefined;

async function play(step: Step, script: Script): Promise<Problem> {
  const socket = script.server.socket();
  switch (step.verb) {
    case "send": {
      socket.passing = step.frame.type;
      try {
        const problem = ask(step.frame, script);
        if (problem !== undefined) return problem;
        for (let n = 0; n < step.repeat; n++) {
          const { stream, type } = step.frame;
          const window = type === PONG ? 0 : SEND_WITHIN;
          if (!(await within(window, () => socket.has(stream, type)))) {
            return `the client sent nothing on stream ${stream} within ${window} ms`;
          }
          const problem = mismatch(step.frame, socket.take(stream, type) as WsFrame);
          if (problem !== undefined) return problem;
        }
      } finally {
        socket.passing = undefined;
      }
      return undefined;
    }
    case "expect":
      if (!socket.deliver(step.frame)) return ENDED;
      await settle();
      return undefined;
    case "wait":
      await vi.advanceTimersByTimeAsync(step.ms);
      return undefined;
    case "expect_nothing_within": {
      if (socket.dropped) return ENDED;
      await vi.advanceTimersByTimeAsync(step.ms);
      if (socket.unread !== 0) return `the client sent ${socket.unread} frames`;
      return socket.closedWith === undefined ? undefined : `the client closed its transport with ${socket.closedWith}`;
    }
    case "close_transport":
      if (!socket.drop(ABNORMAL_CLOSURE)) return ENDED;
      await settle();
      return undefined;
    case "expect_close": {
      const wanted = 4000 + step.code;
      if (!(await within(step.within, () => socket.closedWith !== undefined))) {
        return `the client did not close its transport within ${step.within} ms`;
      }
      return socket.closedWith === wanted ? undefined : `closed with ${socket.closedWith}, not ${wanted}`;
    }
  }
}

function ask(frame: FrameSpec, script: Script): Problem {
  switch (frame.type) {
    case CLOSE:
      return askToClose(frame, script.connection());
    case CALL:
      return askToCall(frame, script);
    case CANCEL: {
      const started = script.calls.get(frame.stream);
      if (started === undefined) return `a CANCEL send needs a call on stream ${frame.stream}`;
      if (!started.deadline) started.abort.abort();
      return undefined;
    }
    default:
      return undefined;
  }
}

function askToCall(frame: FrameSpec, script: Script): Problem {
  const connection = script.connection();
  if (connection === undefined) return "a CALL send needs an established session";
  const method = entry(frame.header, Key.METHOD);
  const deadline = entry(frame.header, Key.DEADLINE);
  const meta = entry(frame.header, Key.META);
  const args = frame.data.kind === "cbor" ? frame.data.diag : undefined;
  if (method?.kind !== "text") return "a CALL send needs a concrete method (key 2)";
  if (deadline !== undefined && deadline.kind !== "uint") return "a CALL send's deadline (key 4) is not a uint";
  if (args?.kind !== "array") return "a CALL send's data is not an argument array";
  const metaInput = scriptMeta(meta);
  if (typeof metaInput === "string") return metaInput;
  const signature: CallSignature = {
    method: method.value,
    params: args.items.map(() => "any"),
    returns: "any",
    error: "any",
    types: {},
  };
  const abort = new AbortController();
  let trailers: unknown;
  const outcome = connection
    .call(signature, args.items.map(value), {
      signal: abort.signal,
      ...(deadline === undefined ? {} : { deadline: Number(deadline.value) }),
      ...(metaInput === undefined ? {} : { meta: metaInput }),
      onTrailers: (received) => (trailers = received),
    })
    .then(
      (result): CallOutcome => ({ ok: true, value: result, trailers }),
      (error: unknown): CallOutcome => ({ ok: false, error }),
    );
  script.calls.set(frame.stream, { outcome, abort, deadline: deadline !== undefined });
  return undefined;
}

function scriptMeta(meta: Diag | undefined): Map<string, string | Uint8Array> | undefined | string {
  if (meta === undefined) return undefined;
  if (meta.kind !== "map") return "a CALL send's meta (key 3) is not a map";
  const out = new Map<string, string | Uint8Array>();
  for (const [name, item] of meta.entries) {
    if (name.kind !== "text" || (item.kind !== "text" && item.kind !== "bytes")) {
      return "a CALL send's meta (key 3) maps text to text or bytes";
    }
    out.set(name.value, item.value);
  }
  return out;
}

function askToClose(frame: FrameSpec, connection: Connection | undefined): string | undefined {
  if (connection === undefined) return "a CLOSE send needs an established session";
  const code = entry(frame.header, Key.CODE);
  const msg = entry(frame.header, Key.MSG);
  if (code?.kind !== "uint") return "a CLOSE send needs a concrete code (key 6)";
  if (msg !== undefined && msg.kind !== "text") return "a CLOSE send's msg (key 7) is not concrete text";
  void connection.close(Number(code.value) as CloseCode, msg?.value);
  return undefined;
}

function helloOptions(header: Diag | undefined): Pick<ConnectOptions, "codecs" | "caps" | "contract"> {
  const texts = (d: Diag | undefined) =>
    d?.kind === "array" && d.items.every((item) => item.kind === "text")
      ? d.items.map((item) => (item as { value: string }).value)
      : undefined;
  const codecs = texts(entry(header, Key.CODECS));
  const caps = texts(entry(header, Key.CAPS));
  const contract = entry(header, Key.CONTRACT);
  return {
    ...(codecs === undefined ? {} : { codecs: codecs as Codec[] }),
    ...(caps === undefined ? {} : { caps }),
    ...(contract?.kind === "bytes" ? { contract: contract.value } : {}),
  };
}

function entry(map: Diag | undefined, key: number): Diag | undefined {
  if (map?.kind !== "map") return undefined;
  return map.entries.find(([k]) => k.kind === "uint" && k.value === BigInt(key))?.[1];
}

function mismatch(spec: FrameSpec, got: WsFrame): string | undefined {
  const { frame } = got;
  if (frame.type !== spec.type) return `got frame type ${frame.type}, not ${spec.type}`;
  if (frame.flags !== spec.flags) return `got flags ${frame.flags}, not ${spec.flags}`;
  if (spec.header === undefined) {
    if (frame.header.length !== 0) return "got a header, expected none";
  } else if (frame.header.length === 0 || !matches(spec.header, decode(frame.header))) {
    return `header does not match: got ${frame.header.length} bytes`;
  }
  switch (spec.data.kind) {
    case "empty":
      return frame.data.length === 0 ? undefined : `got ${frame.data.length} data bytes, expected none`;
    case "exact":
      return sameBytes(frame.data, spec.data.bytes) ? undefined : "data bytes differ";
    case "cbor":
      return frame.data.length !== 0 && matches(spec.data.diag, decode(frame.data)) ? undefined : "data does not match";
  }
}

/** Whether `value` matches `pattern`: `*`, open maps, map entries in any order. */
function matches(pattern: Diag, value: Diag): boolean {
  switch (pattern.kind) {
    case "any":
      return true;
    case "ref":
      return false;
    case "array":
      return (
        value.kind === "array" &&
        pattern.items.length === value.items.length &&
        pattern.items.every((item, i) => matches(item, value.items[i] as Diag))
      );
    case "map":
      return (
        value.kind === "map" &&
        (pattern.open || pattern.entries.length === value.entries.length) &&
        assign(pattern.entries, value.entries, new Array<boolean>(value.entries.length).fill(false))
      );
    case "tag":
      return value.kind === "tag" && value.tag === pattern.tag && matches(pattern.value, value.value);
    default:
      return same(pattern, value);
  }
}

function assign(entries: readonly [Diag, Diag][], values: readonly [Diag, Diag][], used: boolean[]): boolean {
  const [first, ...rest] = entries;
  if (first === undefined) return true;
  for (const [index, [key, value]] of values.entries()) {
    if (used[index] || !matches(first[0], key) || !matches(first[1], value)) continue;
    used[index] = true;
    if (assign(rest, values, used)) return true;
    used[index] = false;
  }
  return false;
}

function dataBytes(data: Data): Uint8Array {
  switch (data.kind) {
    case "empty":
      return new Uint8Array();
    case "exact":
      return data.bytes;
    case "cbor":
      return encode(data.diag);
  }
}

function sameBytes(a: Uint8Array, b: Uint8Array): boolean {
  return a.length === b.length && a.every((byte, i) => byte === b[i]);
}

async function settle(): Promise<void> {
  await vi.advanceTimersByTimeAsync(0);
}

async function within(ms: number, done: () => boolean): Promise<boolean> {
  const deadline = Date.now() + ms;
  await settle();
  while (!done()) {
    if (vi.getTimerCount() === 0 || Date.now() >= deadline) return false;
    await vi.advanceTimersToNextTimerAsync();
    if (Date.now() > deadline) return false;
  }
  return true;
}

function parseStep(step: Json, codec: string, where: string): Step {
  const entries = Object.entries(step);
  if (entries.length !== 1) throw new Error(`${where}: a step has exactly one verb`);
  const [verb, body] = entries[0] as [string, Json];
  switch (verb) {
    case "send":
      return { verb, frame: parseFrame(body, codec, where, true), repeat: (body["repeat"] as number | undefined) ?? 1 };
    case "expect":
      return { verb, frame: parseFrame(body, codec, where, false) };
    case "expect_nothing_within":
    case "wait":
      return { verb, ms: body["ms"] as number };
    case "close_transport":
      return { verb };
    case "expect_close": {
      const name = String(body["code"]);
      if (!Object.hasOwn(CloseCode, name)) throw new Error(`${where}: unknown close code ${name}`);
      return { verb, code: CloseCode[name as keyof typeof CloseCode], within: body["within_ms"] as number };
    }
    default:
      throw new Error(`${where}: unknown verb ${verb}`);
  }
}

function parseFrame(body: Json, caseCodec: string, where: string, patterns: boolean): FrameSpec {
  if (body["capture"] !== undefined) throw new Error(`${where}: a client script captures nothing`);
  const concrete = patterns ? (body["type"] === CALL ? "a CALL send" : undefined) : "an expect";
  const diag = (text: string): Diag => {
    const d = parse(text);
    if (concrete !== undefined && patternCount(d) !== 0) {
      throw new Error(`${where}: ${concrete} is concrete: no *, ... or $name`);
    }
    if (patterns && refs(d).length !== 0) throw new Error(`${where}: a client script uses no $name`);
    return d;
  };
  const codec = (body["codec"] as string | undefined) ?? caseCodec;
  if (codec !== "cbor" && codec !== "json") throw new Error(`${where}: unknown codec ${codec}`);
  const data = dataOf(body["data"], body["data_hex"], codec, diag);
  const header = body["header"];
  return {
    stream: body["stream"] as number,
    type: body["type"] as number,
    flags: (body["flags"] as number | undefined) ?? 0,
    header: typeof header === "string" ? diag(header) : undefined,
    data,
  };
}

function dataOf(text: unknown, dataHex: unknown, codec: Codec, diag: (text: string) => Diag): Data {
  if (typeof text === "string") {
    if (codec === "json") return { kind: "exact", bytes: new TextEncoder().encode(text) };
    return { kind: "cbor", diag: diag(text) };
  }
  if (typeof dataHex === "string") return { kind: "exact", bytes: hexToBytes(dataHex) };
  return { kind: "empty" };
}

/** The server's end of the in-memory WebSocket. */
class ScriptServer {
  private current: ScriptSocket | undefined;

  constructorFor(): new (url: string, protocols: string | string[]) => WebSocketLike {
    const register = (socket: ScriptSocket) => {
      this.current = socket;
    };
    return class extends ScriptSocket {
      constructor() {
        super();
        register(this);
      }
    };
  }

  socket(): ScriptSocket {
    if (this.current === undefined) throw new Error("the client opened no WebSocket");
    return this.current;
  }
}

class ScriptSocket implements WebSocketLike {
  binaryType = "blob";
  protocol = "";
  onopen: ((event: unknown) => void) | null = null;
  onmessage: ((event: { readonly data: unknown }) => void) | null = null;
  onclose: ((event: { readonly code: number }) => void) | null = null;
  onerror: ((event: unknown) => void) | null = null;
  passing: number | undefined;
  closedWith: number | undefined;
  dropped = false;
  private readonly inbox: WsFrame[] = [];

  open(protocol: string): void {
    this.protocol = protocol;
    this.onopen?.({});
  }

  send(data: Uint8Array): void {
    for (const frame of decodeMessage(data, DEFAULT_MAX_FRAME)) {
      const { type } = frame.frame;
      if (type === PING && this.passing !== PING) {
        queueMicrotask(() => this.reply(frame.stream, { ...frame.frame, type: PONG }));
        continue;
      }
      this.inbox.push(frame);
    }
  }

  close(code?: number): void {
    this.closedWith ??= code;
  }

  get unread(): number {
    return this.inbox.filter((received) => !HEARTBEATS.has(received.frame.type)).length;
  }

  has(stream: number, type: number): boolean {
    return this.index(stream, type) >= 0;
  }

  take(stream: number, type: number): WsFrame | undefined {
    const index = this.index(stream, type);
    return index < 0 ? undefined : this.inbox.splice(index, 1)[0];
  }

  private index(stream: number, type: number): number {
    const wanted = (frame: RawFrame) => frame.type === type || !HEARTBEATS.has(frame.type);
    return this.inbox.findIndex((received) => received.stream === stream && wanted(received.frame));
  }

  deliver(spec: FrameSpec): boolean {
    const header = spec.header === undefined ? new Uint8Array() : encode(spec.header);
    return this.reply(spec.stream, { type: spec.type, flags: spec.flags, header, data: dataBytes(spec.data) });
  }

  drop(code: number): boolean {
    if (this.closedWith !== undefined || this.dropped) return false;
    this.dropped = true;
    this.onclose?.({ code });
    return true;
  }

  private reply(stream: number, frame: RawFrame): boolean {
    if (this.closedWith !== undefined || this.dropped) return false;
    for (const message of encodeMessages([{ stream, frame }], DEFAULT_MAX_FRAME)) {
      this.onmessage?.({ data: arrayBufferOf(message) });
    }
    return true;
  }
}
