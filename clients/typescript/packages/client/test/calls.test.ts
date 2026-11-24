import { runInNewContext } from "node:vm";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  type CallSignature,
  CloseCode,
  type ConnectOptions,
  type Connection,
  encodeData,
  type Frame,
  StatusCode,
  StatusError,
} from "../src/index.js";
import { established, FakeSocket } from "./fake-socket.js";
import { welcome } from "./support.js";

type Extra = Partial<ConnectOptions>;

const TYPES = { Problem: { struct: { fields: [{ name: "reason", type: "string" as const }] } } };

const SEND: CallSignature = {
  method: "chat/send",
  params: ["string", "u32"],
  returns: "u64",
  error: { ref: "Problem" },
  types: TYPES,
};

const PING: CallSignature = { method: "ping", params: [], returns: "unit", types: {} };

const QUIET = { hb: 60_000n, idle: 600_000n };

function data(type: CallSignature["returns"], value: unknown): Uint8Array {
  return encodeData(type, TYPES, value, "cbor");
}

function result(value: unknown, type: CallSignature["returns"] = "u64", meta?: Map<string, string>): Frame {
  return { type: "RESULT", data: data(type, value), ...(meta === undefined ? {} : { meta }) };
}

function error(code: bigint, fields: Partial<Extract<Frame, { type: "ERROR" }>> = {}): Frame {
  return { type: "ERROR", code, msg: "no", data: new Uint8Array(), ...fields };
}

async function open(fields = {}, extra: Extra = {}) {
  const { connection, ws } = await established(extra, { ...QUIET, ...fields });
  vi.advanceTimersByTime(0);
  return { connection, ws };
}

function callsOn(ws: FakeSocket): { stream: number; frame: Frame }[] {
  return ws.sentOn.filter(({ frame }) => frame.type === "CALL" || frame.type === "CANCEL");
}

async function failure(promise: Promise<unknown>): Promise<StatusError> {
  const outcome = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(outcome).toBeInstanceOf(StatusError);
  return outcome as StatusError;
}

async function settledWithin(promise: Promise<unknown>): Promise<boolean> {
  let done = false;
  promise.then(
    () => (done = true),
    () => (done = true),
  );
  await vi.advanceTimersByTimeAsync(0);
  return done;
}

beforeEach(() => {
  vi.useFakeTimers();
  FakeSocket.last = undefined;
  FakeSocket.count = 0;
});

afterEach(() => {
  vi.useRealTimers();
});

describe("call", () => {
  it("sends CALL with the method, shape 0 and the arguments, and resolves with the RESULT", async () => {
    const { connection, ws } = await open();
    const pending = connection.call(SEND, ["hi", 7]);
    const args = encodeData({ tuple: ["string", "u32"] }, {}, ["hi", 7], "cbor");
    expect(callsOn(ws)).toEqual([{ stream: 2, frame: { type: "CALL", method: "chat/send", shape: 0n, data: args } }]);
    ws.receive(result(42n), 2);
    await expect(pending).resolves.toBe(42n);
  });

  it("sends an empty array for a method without parameters and decodes unit as null", async () => {
    const { connection, ws } = await open();
    const pending = connection.call(PING, []);
    expect(callsOn(ws)[0]?.frame).toMatchObject({ type: "CALL", method: "ping", data: new Uint8Array([0x80]) });
    ws.receive(result(null, "unit"), 2);
    await expect(pending).resolves.toBeNull();
  });

  it("uses stream IDs 2 and 4 for two calls at once, and answers each on its own stream", async () => {
    const { connection, ws } = await open();
    const first = connection.call(SEND, ["a", 1]);
    const second = connection.call(SEND, ["b", 2]);
    expect(callsOn(ws).map(({ stream }) => stream)).toEqual([2, 4]);
    ws.receive(result(4n), 4);
    ws.receive(result(2n), 2);
    await expect(Promise.all([first, second])).resolves.toEqual([2n, 4n]);
  });

  it("sends CANCEL on abort, rejects with CANCELLED, and keeps the stream until the terminal frame", async () => {
    const { connection, ws } = await open({ limits: { ...welcome().limits, maxCalls: 1n } });
    const controller = new AbortController();
    const pending = connection.call(SEND, ["a", 1], { signal: controller.signal });
    controller.abort();
    expect(callsOn(ws).at(-1)).toEqual({ stream: 2, frame: { type: "CANCEL" } });
    const cancelled = await failure(pending);
    expect([cancelled.code, cancelled.nx]).toEqual([StatusCode.CANCELLED, false]);
    const busy = await failure(connection.call(SEND, ["b", 2]));
    expect([busy.code, busy.nx]).toEqual([StatusCode.RESOURCE_EXHAUSTED, true]);
    ws.receive({ type: "ITEM", data: new Uint8Array([0x01]) }, 2);
    ws.receive(error(BigInt(StatusCode.CANCELLED)), 2);
    expect(connection.closedWith).toBeUndefined();
    const next = connection.call(SEND, ["c", 3]);
    expect(callsOn(ws).at(-1)?.stream).toBe(4);
    ws.receive(result(3n), 4);
    await expect(next).resolves.toBe(3n);
  });

  it("sends nothing for a signal already aborted, and rejects with CANCELLED and nx", async () => {
    const { connection, ws } = await open();
    const sent = ws.sent.length;
    const cancelled = await failure(connection.call(SEND, ["a", 1], { signal: AbortSignal.abort() }));
    expect([cancelled.code, cancelled.nx, ws.sent.length]).toEqual([StatusCode.CANCELLED, true, sent]);
  });

  it("sends the deadline and rejects with DEADLINE_EXCEEDED after it, sending CANCEL", async () => {
    const { connection, ws } = await open();
    const pending = connection.call(SEND, ["a", 1], { deadline: 99.5 });
    expect(callsOn(ws)[0]?.frame).toMatchObject({ type: "CALL", deadline: 100n });
    await vi.advanceTimersByTimeAsync(99);
    expect(await settledWithin(pending)).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    const expired = await failure(pending);
    expect([expired.code, expired.nx]).toEqual([StatusCode.DEADLINE_EXCEEDED, false]);
    expect(callsOn(ws).at(-1)).toEqual({ stream: 2, frame: { type: "CANCEL" } });
  });

  it("expires each call at its own deadline", async () => {
    const { connection } = await open();
    const late = failure(connection.call(SEND, ["a", 1], { deadline: 100 }));
    const early = failure(connection.call(SEND, ["b", 2], { deadline: 50 }));
    await vi.advanceTimersByTimeAsync(50);
    expect((await early).code).toBe(StatusCode.DEADLINE_EXCEEDED);
    expect(await settledWithin(late)).toBe(false);
    await vi.advanceTimersByTimeAsync(50);
    expect((await late).code).toBe(StatusCode.DEADLINE_EXCEEDED);
  });

  it("removes its abort listener however the call ends", async () => {
    const { connection, ws } = await open();
    const ends: ((stream: number) => void)[] = [
      (stream) => ws.receive(result(1n), stream),
      (stream) => ws.receive(error(5n), stream),
      () => vi.advanceTimersByTime(100),
      () => ws.receive({ type: "CLOSE", code: 0n }),
    ];
    for (const [index, end] of ends.entries()) {
      const signal = new AbortController().signal;
      const removed = vi.spyOn(signal, "removeEventListener");
      const pending = connection.call(SEND, ["a", 1], { signal, deadline: 100 }).catch(() => {});
      end(2 + 2 * index);
      await pending;
      expect(removed, `end ${index}`).toHaveBeenCalledWith("abort", expect.any(Function));
    }
  });

  it("cancels a call whose signal aborts while the call is being set up", async () => {
    const { connection, ws } = await open();
    const controller = new AbortController();
    const meta = {
      get "x-id"() {
        controller.abort();
        return "1";
      },
    };
    const cancelled = await failure(connection.call(SEND, ["a", 1], { signal: controller.signal, meta }));
    expect([cancelled.code, cancelled.nx]).toEqual([StatusCode.CANCELLED, false]);
    expect(callsOn(ws).map(({ frame }) => frame.type)).toEqual(["CALL", "CANCEL"]);
  });

  it("clears the deadline and the abort listener once the call completes", async () => {
    const { connection, ws } = await open();
    const controller = new AbortController();
    const pending = connection.call(SEND, ["a", 1], { deadline: 100, signal: controller.signal });
    ws.receive(result(1n), 2);
    await expect(pending).resolves.toBe(1n);
    controller.abort();
    await vi.advanceTimersByTimeAsync(200);
    expect(callsOn(ws).filter(({ frame }) => frame.type === "CANCEL")).toEqual([]);
  });

  it("refuses a deadline that is not a positive number of milliseconds", async () => {
    const { connection } = await open();
    for (const deadline of [0, -1, Number.NaN, Number.POSITIVE_INFINITY, 2 ** 60]) {
      await expect(connection.call(SEND, ["a", 1], { deadline })).rejects.toThrow(RangeError);
    }
  });

  it("hands RESULT trailers to onTrailers and puts ERROR meta on the error", async () => {
    const { connection, ws } = await open();
    const trailers: unknown[] = [];
    const ok = connection.call(SEND, ["a", 1], { onTrailers: (meta) => trailers.push(meta) });
    ws.receive(result(1n, "u64", new Map([["x-cost", "3"]])), 2);
    await expect(ok).resolves.toBe(1n);
    expect(trailers).toEqual([new Map([["x-cost", "3"]])]);
    const failed = connection.call(SEND, ["b", 2]);
    ws.receive(error(5n, { meta: new Map([["x-trace", "t"]]) }), 4);
    expect((await failure(failed)).meta).toEqual(new Map([["x-trace", "t"]]));
  });

  it("rejects with what onTrailers throws", async () => {
    const { connection, ws } = await open();
    const boom = new Error("boom");
    const pending = connection.call(SEND, ["a", 1], {
      onTrailers: () => {
        throw boom;
      },
    });
    ws.receive(result(1n, "u64", new Map([["x-cost", "3"]])), 2);
    await expect(pending).rejects.toBe(boom);
  });

  it("sends call metadata and refuses a name outside [a-z0-9-] or a value that is not text or bytes", async () => {
    const { connection, ws } = await open();
    void connection.call(SEND, ["a", 1], { meta: { "x-id": "1", "x-raw": new Uint8Array([1]) } });
    expect(callsOn(ws)[0]?.frame).toMatchObject({
      meta: new Map<string, string | Uint8Array>([
        ["x-id", "1"],
        ["x-raw", new Uint8Array([1])],
      ]),
    });
    const sent = ws.sent.length;
    const bad = await failure(connection.call(SEND, ["a", 1], { meta: { "X-Bad": "1" } }));
    expect([bad.code, bad.nx, ws.sent.length]).toEqual([StatusCode.INVALID_ARGUMENT, true, sent]);
    const value = await failure(connection.call(SEND, ["a", 1], { meta: new Map([["x-n", 1 as never]]) }));
    expect([value.code, value.nx, ws.sent.length]).toEqual([StatusCode.INVALID_ARGUMENT, true, sent]);
    const key = await failure(connection.call(SEND, ["a", 1], { meta: new Map([[1 as never, "x"]]) }));
    expect([key.code, key.nx, key.message, ws.sent.length]).toEqual([
      StatusCode.INVALID_ARGUMENT,
      true,
      "metadata name 1 is not [a-z0-9-] text",
      sent,
    ]);
  });

  it("reads metadata from a Map of another realm", async () => {
    const { connection, ws } = await open();
    const foreign = runInNewContext('new Map([["x-id", "1"]])') as Map<string, string>;
    expect(foreign instanceof Map).toBe(false);
    void connection.call(SEND, ["a", 1], { meta: foreign });
    expect(callsOn(ws)[0]?.frame).toMatchObject({ meta: new Map([["x-id", "1"]]) });
    const fake = { [Symbol.toStringTag]: "Map", entries: () => [["x-id", "1"]].values() };
    const refused = await failure(connection.call(SEND, ["b", 2], { meta: fake as never }));
    expect([refused.code, refused.nx, callsOn(ws).length]).toEqual([StatusCode.INVALID_ARGUMENT, true, 1]);
  });

  it("refuses arguments of the wrong count or type before sending", async () => {
    const { connection, ws } = await open();
    const sent = ws.sent.length;
    for (const args of [["a"], ["a", 1, 2], ["a", "not a number"]]) {
      const refused = await failure(connection.call(SEND, args));
      expect([refused.code, refused.nx]).toEqual([StatusCode.INVALID_ARGUMENT, true]);
    }
    expect(ws.sent.length).toBe(sent);
  });

  it("refuses a CALL whose header is above 16 KiB before sending, and leaves its stream ID unopened", async () => {
    const { connection, ws } = await open();
    const sent = ws.sent.length;
    const refused = await failure(connection.call(SEND, ["a", 1], { meta: { "x-big": "x".repeat(17_000) } }));
    expect([refused.code, refused.nx, ws.sent.length]).toEqual([StatusCode.INVALID_ARGUMENT, true, sent]);
    ws.receive(result(1n), 2);
    expect(connection.closedWith).toMatchObject({ code: CloseCode.PROTOCOL_ERROR, by: "local" });
  });

  it("gives the next call the stream ID a refused CALL did not take", async () => {
    const { connection, ws } = await open();
    void connection.call(SEND, ["a", 1], { meta: { "x-big": "x".repeat(17_000) } }).catch(() => {});
    const next = connection.call(SEND, ["b", 2]);
    expect(callsOn(ws).map(({ stream }) => stream)).toEqual([2]);
    ws.receive(result(2n), 2);
    await expect(next).resolves.toBe(2n);
  });

  it("measures the local deadline from the call, before its arguments are encoded", async () => {
    const { connection } = await open();
    const slow: CallSignature = {
      method: "slow",
      params: [{ struct: { fields: [{ name: "text", type: "string" }] } }],
      returns: "unit",
      types: {},
    };
    const started = Date.now();
    const arg = {
      get text() {
        vi.setSystemTime(started + 60);
        return "x";
      },
    };
    const pending = failure(connection.call(slow, [arg], { deadline: 100 }));
    await vi.advanceTimersByTimeAsync(39);
    expect(await settledWithin(pending)).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    expect((await pending).code).toBe(StatusCode.DEADLINE_EXCEEDED);
  });

  it("refuses a call beyond WELCOME max_calls with RESOURCE_EXHAUSTED and nx", async () => {
    const { connection, ws } = await open({ limits: { ...welcome().limits, maxCalls: 2n } });
    void connection.call(SEND, ["a", 1]);
    void connection.call(SEND, ["b", 2]);
    const sent = ws.sent.length;
    const refused = await failure(connection.call(SEND, ["c", 3]));
    expect([refused.code, refused.nx, ws.sent.length]).toEqual([StatusCode.RESOURCE_EXHAUSTED, true, sent]);
  });

  it("exposes an ERROR's code, message, typed detail and nx", async () => {
    const { connection, ws } = await open();
    const pending = connection.call(SEND, ["a", 1]);
    ws.receive(error(7n, { msg: "denied", nx: true, data: data({ ref: "Problem" }, { reason: "banned" }) }), 2);
    const denied = await failure(pending);
    expect([denied.code, denied.message, denied.detail, denied.nx]).toEqual([
      StatusCode.PERMISSION_DENIED,
      "denied",
      { reason: "banned" },
      true,
    ]);
  });

  it("maps an unknown status code and OK in an ERROR to UNKNOWN", async () => {
    const { connection, ws } = await open();
    for (const [stream, code] of [
      [2, 99n],
      [4, 0n],
    ] as const) {
      const pending = connection.call(SEND, ["a", 1]);
      ws.receive(error(code), stream);
      expect((await failure(pending)).code).toBe(StatusCode.UNKNOWN);
    }
  });

  it("completes with INTERNAL on a RESULT or ERROR detail that does not decode, and goes on", async () => {
    const { connection, ws } = await open();
    const undecodable = connection.call(SEND, ["a", 1]);
    ws.receive({ type: "RESULT", data: data("string", "not a u64") }, 2);
    const internal = await failure(undecodable);
    expect([internal.code, internal.nx]).toEqual([StatusCode.INTERNAL, false]);
    const badDetail = connection.call(SEND, ["b", 2]);
    ws.receive(error(9n, { data: data("u64", 1n) }), 4);
    expect((await failure(badDetail)).code).toBe(StatusCode.INTERNAL);
    const noErrorType = connection.call(PING, []);
    ws.receive(error(9n, { data: data("u64", 1n) }), 6);
    expect((await failure(noErrorType)).code).toBe(StatusCode.INTERNAL);
    const next = connection.call(SEND, ["c", 3]);
    ws.receive(result(8n), 8);
    await expect(next).resolves.toBe(8n);
    expect(connection.closedWith).toBeUndefined();
  });

  it("rejects open calls with UNAVAILABLE when the session ends, and later calls with nx", async () => {
    const { connection, ws } = await open();
    const pending = connection.call(SEND, ["a", 1], { deadline: 1000 });
    ws.receive({ type: "CLOSE", code: 7n });
    const lost = await failure(pending);
    expect([lost.code, lost.nx]).toEqual([StatusCode.UNAVAILABLE, false]);
    const late = await failure(connection.call(SEND, ["b", 2]));
    expect([late.code, late.nx]).toEqual([StatusCode.UNAVAILABLE, true]);
  });
});

describe("frames on client streams", () => {
  function closesWithProtocolError(connection: Connection): void {
    expect(connection.closedWith).toMatchObject({ kind: "closed", by: "local", code: CloseCode.PROTOCOL_ERROR });
  }

  it("closes the session for a frame on a client stream not yet opened", async () => {
    const { connection, ws } = await open();
    ws.receive(result(1n), 2);
    closesWithProtocolError(connection);
  });

  it("ignores a frame for a call stream that has completed", async () => {
    const { connection, ws } = await open();
    const pending = connection.call(SEND, ["a", 1]);
    ws.receive(result(1n), 2);
    await pending;
    ws.receive(result(2n), 2);
    ws.receive({ type: "CREDIT", credit: 10n } as Frame, 2);
    expect(connection.closedWith).toBeUndefined();
  });

  it("closes for a server CALL or LANE_OPEN on an open, a cancelled or a completed client stream", async () => {
    const caps = { caps: ["server-calls", "lanes"] };
    const openings: Frame[] = [
      { type: "CALL", method: "x", shape: 0n, data: new Uint8Array([0x80]) },
      { type: "LANE_OPEN", lane: 1n },
    ];
    const before: ((connection: Connection, ws: FakeSocket) => Promise<void>)[] = [
      async (connection) => void connection.call(SEND, ["a", 1]).catch(() => {}),
      async (connection) => {
        const controller = new AbortController();
        void connection.call(SEND, ["a", 1], { signal: controller.signal }).catch(() => {});
        controller.abort();
      },
      async (connection, ws) => {
        const done = connection.call(SEND, ["a", 1]);
        ws.receive(result(1n), 2);
        await done;
      },
    ];
    for (const opening of openings) {
      for (const setUp of before) {
        const { connection, ws } = await open(caps, caps);
        await setUp(connection, ws);
        await vi.advanceTimersByTimeAsync(5);
        ws.receive(opening, 2);
        closesWithProtocolError(connection);
      }
    }
  });

  it("ignores CREDIT on an open call and closes for an ITEM, END, CALL or CANCEL on it", async () => {
    for (const frame of [
      { type: "ITEM", data: new Uint8Array([1]) },
      { type: "END" },
      { type: "CALL", method: "x", shape: 0n, data: new Uint8Array([0x80]) },
      { type: "CANCEL" },
    ] as Frame[]) {
      const { connection, ws } = await open();
      void connection.call(SEND, ["a", 1]).catch(() => {});
      ws.receive({ type: "CREDIT", credit: 10n } as Frame, 2);
      expect(connection.closedWith).toBeUndefined();
      ws.receive(frame, 2);
      closesWithProtocolError(connection);
    }
  });
});
