import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { encodeData, type NotifySignature, StatusCode, StatusError } from "../src/index.js";
import { established, FakeSocket } from "./fake-socket.js";

const TYPING: NotifySignature = { method: "chat/typing", params: ["string", "u8"], types: {} };

const QUIET = { hb: 60_000n, idle: 600_000n };

async function open() {
  const { connection, ws } = await established({}, QUIET);
  vi.advanceTimersByTime(0);
  return { connection, ws };
}

function refusal(send: () => void): StatusError {
  let caught: unknown;
  try {
    send();
  } catch (error) {
    caught = error;
  }
  expect(caught).toBeInstanceOf(StatusError);
  return caught as StatusError;
}

beforeEach(() => {
  vi.useFakeTimers();
  FakeSocket.last = undefined;
  FakeSocket.count = 0;
});

afterEach(() => {
  vi.useRealTimers();
});

describe("notify", () => {
  it("sends NOTIFY on stream 0 with the method and the arguments, and returns nothing", async () => {
    const { connection, ws } = await open();
    expect(connection.notify(TYPING, ["lobby", 3])).toBeUndefined();
    const data = encodeData({ tuple: ["string", "u8"] }, {}, ["lobby", 3], "cbor");
    expect(ws.sentOn.filter(({ frame }) => frame.type === "NOTIFY")).toEqual([
      { stream: 0, frame: { type: "NOTIFY", method: "chat/typing", data } },
    ]);
  });

  it("sends notification metadata", async () => {
    const { connection, ws } = await open();
    connection.notify(TYPING, ["lobby", 3], { meta: { trace: "t1" } });
    const sent = ws.sentOn.filter(({ frame }) => frame.type === "NOTIFY");
    expect(sent.map(({ frame }) => frame)).toMatchObject([{ meta: new Map([["trace", "t1"]]) }]);
  });

  it("throws INVALID_ARGUMENT nx, sending nothing, for bad metadata, a wrong count or a wrong type", async () => {
    const { connection, ws } = await open();
    const sent = ws.sentOn.length;
    for (const [args, meta] of [
      [["lobby", 3], { Trace: "t1" }],
      [["lobby"], undefined],
      [["lobby", 300], undefined],
    ] as const) {
      const error = refusal(() => connection.notify(TYPING, args, meta === undefined ? {} : { meta }));
      expect([error.code, error.nx]).toEqual([StatusCode.INVALID_ARGUMENT, true]);
    }
    expect(ws.sentOn.length).toBe(sent);
  });

  it("throws INVALID_ARGUMENT nx, sending nothing, for a NOTIFY whose header is above 16 KiB", async () => {
    const { connection, ws } = await open();
    const sent = ws.sent.length;
    const error = refusal(() => connection.notify(TYPING, ["lobby", 3], { meta: { "x-big": "x".repeat(17_000) } }));
    expect([error.code, error.nx, error.message.startsWith("the NOTIFY does not encode"), ws.sent.length]).toEqual([
      StatusCode.INVALID_ARGUMENT,
      true,
      true,
      sent,
    ]);
  });

  it("throws UNAVAILABLE nx once the session has ended", async () => {
    const { connection, ws } = await open();
    ws.receive({ type: "CLOSE", code: 7n });
    const error = refusal(() => connection.notify(TYPING, ["lobby", 3]));
    expect([error.code, error.nx]).toEqual([StatusCode.UNAVAILABLE, true]);
  });
});
