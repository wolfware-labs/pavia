import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CloseCode, ConnectError, connect, type Ending } from "../src/index.js";
import { established, FakeSocket, options, socket } from "./fake-socket.js";
import { CONTRACT, concat, encodeError, HB, IDLE, message, welcome } from "./support.js";

beforeEach(() => {
  vi.useFakeTimers();
  FakeSocket.last = undefined;
  FakeSocket.count = 0;
});

afterEach(() => {
  vi.useRealTimers();
});

describe("connect", () => {
  it("requests pavia.1, sends HELLO on open and resolves on WELCOME", async () => {
    const { connection, ws } = await established({ codecs: ["json"] }, { codec: "json" });
    expect(ws.url).toBe("wss://example.test/rt");
    expect(ws.protocols).toBe("pavia.1");
    expect(ws.binaryType).toBe("arraybuffer");
    expect(ws.sent[0]).toMatchObject({ type: "HELLO", codecs: ["json"] });
    expect(connection.welcome.session).toBe("s1");
  });

  it("fails before opening a socket when the options make a HELLO no server accepts", () => {
    expect(encodeError(() => connect("wss://example.test/rt", options({ limits: { maxFrame: 32_768n } }))).kind).toBe(
      "limits_below_floor",
    );
    expect(FakeSocket.count).toBe(0);
  });

  it("rejects on REJECT, on CLOSE before WELCOME and on the handshake timeout", async () => {
    const rejected = connect("wss://example.test/rt", options());
    socket().open();
    socket().receive({ type: "REJECT", code: BigInt(CloseCode.FORBIDDEN), msg: "no" });
    const ending = { kind: "rejected", code: CloseCode.FORBIDDEN, msg: "no" };
    await expect(rejected).rejects.toMatchObject({ ending });

    const closed = connect("wss://example.test/rt", options());
    socket().open();
    socket().drop(1006);
    await expect(closed).rejects.toBeInstanceOf(ConnectError);

    const slow = connect("wss://example.test/rt", options({ handshakeTimeout: 1_000 }));
    socket().open();
    const ws = socket();
    vi.advanceTimersByTime(1_000);
    await expect(slow).rejects.toMatchObject({ ending: { code: CloseCode.HANDSHAKE_TIMEOUT } });
    expect(ws.closedWith?.code).toBe(4000 + CloseCode.HANDSHAKE_TIMEOUT);
  });

  it("treats a WebSocket error as a lost transport, before WELCOME and after it", async () => {
    const pending = connect("wss://example.test/rt", options());
    socket().open();
    socket().onerror?.({});
    await expect(pending).rejects.toMatchObject({ ending: { kind: "transport_lost" } });
    const { connection, ws } = await established();
    ws.onerror?.({});
    ws.drop(1006);
    expect(connection.closedWith).toEqual({ kind: "transport_lost" });
  });

  it("closes with PROTOCOL_ERROR when the server does not select pavia.1", async () => {
    const pending = connect("wss://example.test/rt", options());
    const ws = socket();
    ws.open("");
    await expect(pending).rejects.toMatchObject({ ending: { code: CloseCode.PROTOCOL_ERROR } });
    expect(ws.closedWith?.code).toBe(4000 + CloseCode.PROTOCOL_ERROR);
    expect(ws.sent).toEqual([]);
  });

  it("warns once and dispatches contractMismatch after connect resolves", async () => {
    const warn = vi.fn();
    const remote = new Uint8Array(32).fill(9);
    const { connection } = await established({ contract: CONTRACT, warn }, { contract: remote });
    const seen = vi.fn();
    connection.addEventListener("contractMismatch", seen);
    vi.advanceTimersByTime(0);
    expect(seen).toHaveBeenCalledTimes(1);
    expect((seen.mock.calls[0]?.[0] as CustomEvent).detail).toEqual({ local: CONTRACT, remote });
    expect(warn).toHaveBeenCalledTimes(1);
    expect(warn.mock.calls[0]?.[0]).toContain("07".repeat(32));
    expect(connection.contractMismatch).toEqual({ local: CONTRACT, remote });
    expect(connection.closedWith).toBeUndefined();
  });

  it("does not warn when the fingerprints match or the server sends none", async () => {
    for (const fields of [{ contract: CONTRACT }, {}]) {
      const warn = vi.fn();
      const { connection } = await established({ contract: CONTRACT, warn }, fields);
      const seen = vi.fn();
      connection.addEventListener("contractMismatch", seen);
      vi.advanceTimersByTime(0);
      expect([seen.mock.calls.length, warn.mock.calls.length]).toEqual([0, 0]);
    }
  });
});

describe("connection", () => {
  it("holds events from the WELCOME message until connect has resolved", async () => {
    const pending = connect("wss://example.test/rt", options({ contract: CONTRACT }));
    const ws = socket();
    ws.open();
    const both = new Uint8Array([
      ...message(welcome({ contract: new Uint8Array(32).fill(9) })),
      ...message({ type: "END" }, 1),
      ...message({ type: "CLOSE", code: 0n }),
    ]);
    ws.onmessage?.({ data: both.buffer });
    const connection = await pending;
    const seen: string[] = [];
    for (const type of ["contractMismatch", "frame", "close"]) connection.addEventListener(type, () => seen.push(type));
    vi.advanceTimersByTime(0);
    expect(seen).toEqual(["contractMismatch", "frame", "close"]);
  });

  it("resolves when the WELCOME message goes on with a frame that closes the session", async () => {
    const pending = connect("wss://example.test/rt", options({ contract: CONTRACT }));
    const ws = socket();
    ws.open();
    const data = concat([message(welcome({ contract: new Uint8Array(32).fill(9) })), message({ type: "END" }, 0)]);
    ws.onmessage?.({ data: data.buffer });
    const connection = await pending;
    const seen: string[] = [];
    for (const type of ["contractMismatch", "close"]) connection.addEventListener(type, () => seen.push(type));
    vi.advanceTimersByTime(0);
    expect(seen).toEqual(["contractMismatch", "close"]);
    expect(connection.closedWith).toMatchObject({ kind: "closed", by: "local", code: CloseCode.PROTOCOL_ERROR });
    expect(ws.closedWith?.code).toBe(4000 + CloseCode.PROTOCOL_ERROR);
  });

  it("hands on no held frame after a listener closes the session", async () => {
    const pending = connect("wss://example.test/rt", options());
    const ws = socket();
    ws.open();
    const frames = [message(welcome()), message({ type: "END" }, 1), message({ type: "END" }, 3)];
    const data = concat(frames);
    ws.onmessage?.({ data: data.buffer });
    const connection = await pending;
    const seen: string[] = [];
    connection.addEventListener("frame", (event) => {
      const { stream } = (event as CustomEvent<{ stream: number }>).detail;
      seen.push(`frame ${stream}`);
      void connection.close();
    });
    connection.addEventListener("close", () => seen.push("close"));
    vi.advanceTimersByTime(0);
    expect(seen).toEqual(["frame 1", "close"]);
  });

  it("finishes the handshake when the warn callback throws", async () => {
    const warn = () => {
      throw new Error("boom");
    };
    const { connection } = await established({ contract: CONTRACT, warn }, { contract: new Uint8Array(32).fill(9) });
    expect(connection.contractMismatch).toBeDefined();
  });

  it("keeps timer delays within what setTimeout holds", async () => {
    const spy = vi.spyOn(globalThis, "setTimeout");
    await established({}, { hb: 2n ** 40n, idle: 2n ** 40n });
    const delays = spy.mock.calls.map((call) => call[1] as number);
    expect(Math.max(...delays)).toBeLessThanOrEqual(2 ** 31 - 1);
    spy.mockRestore();
  });


  it("pings after hb, answers PING and closes with IDLE_TIMEOUT when the server goes quiet", async () => {
    const { connection, ws } = await established();
    const ended = vi.fn();
    connection.addEventListener("close", ended);
    ws.receive({ type: "PING", data: Uint8Array.of(5) });
    expect(ws.sent.at(-1)).toEqual({ type: "PONG", data: Uint8Array.of(5) });
    vi.advanceTimersByTime(HB);
    expect(ws.sent.at(-1)).toEqual({ type: "PING", data: new Uint8Array(0) });
    vi.advanceTimersByTime(IDLE - HB);
    expect(ws.closedWith).toEqual({ code: 4000 + CloseCode.IDLE_TIMEOUT, reason: "" });
    expect((ended.mock.calls[0]?.[0] as CustomEvent<Ending>).detail).toEqual({
      kind: "closed",
      code: CloseCode.IDLE_TIMEOUT,
      by: "local",
    });
  });

  it("closes with CLOSE and 4000 + code and resolves with the ending", async () => {
    const { connection, ws } = await established();
    const ending = await connection.close(CloseCode.GOING_AWAY, "deploy");
    expect(ws.sent.at(-1)).toEqual({ type: "CLOSE", code: BigInt(CloseCode.GOING_AWAY), msg: "deploy" });
    expect(ws.closedWith).toEqual({ code: 4000 + CloseCode.GOING_AWAY, reason: "deploy" });
    expect(ending).toEqual({ kind: "closed", code: CloseCode.GOING_AWAY, msg: "deploy", by: "local" });
    expect(connection.closedWith).toEqual(ending);
    expect(connection.welcome.session).toBe("s1");
  });

  it("ends when the server closes, mapping an unknown code to INTERNAL", async () => {
    const { connection, ws } = await established();
    ws.receive({ type: "CLOSE", code: 77n });
    expect(connection.closedWith).toEqual({ kind: "closed", code: CloseCode.INTERNAL, by: "peer" });
    expect(ws.closedWith?.code).toBe(4000 + CloseCode.INTERNAL);
  });

  it("passes frames of the control stream and server streams on as frame events", async () => {
    const { connection, ws } = await established();
    const frames = vi.fn();
    connection.addEventListener("frame", frames);
    vi.advanceTimersByTime(0);
    const goaway = { type: "GOAWAY", code: 7n, msg: "bye" } as const;
    ws.receive(goaway, 0);
    ws.receive({ type: "END" }, 1);
    expect(frames.mock.calls.map(([event]) => (event as CustomEvent).detail)).toEqual([
      { stream: 0, frame: goaway },
      { stream: 1, frame: { type: "END" } },
    ]);
  });

  it("ends with PROTOCOL_ERROR on a text message", async () => {
    const { connection, ws } = await established();
    ws.onmessage?.({ data: "text" });
    expect(connection.closedWith).toMatchObject({ code: CloseCode.PROTOCOL_ERROR, by: "local" });
  });
});
