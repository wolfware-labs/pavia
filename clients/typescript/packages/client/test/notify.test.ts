import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { encodeData, type NotifySignature, StatusCode } from "../src/index.js";
import { establishedQuietly, FakeSocket } from "./fake-socket.js";
import { statusError, welcome } from "./support.js";

const TYPING: NotifySignature = { method: "chat/typing", params: ["string", "u8"], types: {} };

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
    const { connection, ws } = await establishedQuietly();
    expect(connection.notify(TYPING, ["lobby", 3])).toBeUndefined();
    const data = encodeData({ tuple: ["string", "u8"] }, {}, ["lobby", 3], "cbor");
    expect(ws.sentOn.filter(({ frame }) => frame.type === "NOTIFY")).toEqual([
      { stream: 0, frame: { type: "NOTIFY", method: "chat/typing", data } },
    ]);
  });

  it("sends notification metadata", async () => {
    const { connection, ws } = await establishedQuietly();
    connection.notify(TYPING, ["lobby", 3], { meta: { trace: "t1" } });
    const sent = ws.sentOn.filter(({ frame }) => frame.type === "NOTIFY");
    expect(sent.map(({ frame }) => frame)).toMatchObject([{ meta: new Map([["trace", "t1"]]) }]);
  });

  it("throws INVALID_ARGUMENT nx, sending nothing, for bad metadata, a wrong count or a wrong type", async () => {
    const { connection, ws } = await establishedQuietly();
    const sent = ws.sentOn.length;
    for (const [args, meta] of [
      [["lobby", 3], { Trace: "t1" }],
      [["lobby"], undefined],
      [["lobby", 300], undefined],
    ] as const) {
      const error = statusError(() => connection.notify(TYPING, args, meta === undefined ? {} : { meta }));
      expect([error.code, error.nx]).toEqual([StatusCode.INVALID_ARGUMENT, true]);
    }
    expect(ws.sentOn.length).toBe(sent);
  });

  it("throws INVALID_ARGUMENT nx, sending nothing, for a NOTIFY whose header is above 16 KiB", async () => {
    const { connection, ws } = await establishedQuietly();
    const sent = ws.sent.length;
    const error = statusError(() => connection.notify(TYPING, ["lobby", 3], { meta: { "x-big": "x".repeat(17_000) } }));
    expect([error.code, error.nx, error.message.startsWith("the NOTIFY does not encode"), ws.sent.length]).toEqual([
      StatusCode.INVALID_ARGUMENT,
      true,
      true,
      sent,
    ]);
  });

  it("throws INVALID_ARGUMENT nx, sending nothing, for a NOTIFY above the server's max_frame", async () => {
    const { connection, ws } = await establishedQuietly({ limits: { ...welcome().limits, maxFrame: 65_536n } });
    const sent = ws.sent.length;
    const error = statusError(() => connection.notify(TYPING, ["x".repeat(70_000), 3]));
    expect([error.code, error.nx, ws.sent.length]).toEqual([StatusCode.INVALID_ARGUMENT, true, sent]);
  });

  it("throws UNIMPLEMENTED nx, sending nothing, while resume is granted", async () => {
    const resume = { caps: ["resume"], token: new Uint8Array(16).fill(1), resumeWindow: 30_000n };
    const { connection, ws } = await establishedQuietly(resume, { caps: ["resume"] });
    const sent = ws.sent.length;
    const error = statusError(() => connection.notify(TYPING, ["lobby", 3]));
    expect([error.code, error.nx, ws.sent.length]).toEqual([StatusCode.UNIMPLEMENTED, true, sent]);
  });

  it("throws UNAVAILABLE nx once the session has ended", async () => {
    const { connection, ws } = await establishedQuietly();
    ws.receive({ type: "CLOSE", code: 7n });
    const error = statusError(() => connection.notify(TYPING, ["lobby", 3]));
    expect([error.code, error.nx]).toEqual([StatusCode.UNAVAILABLE, true]);
  });
});
