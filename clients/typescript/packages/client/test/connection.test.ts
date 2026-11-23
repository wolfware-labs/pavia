import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  CloseCode,
  ConnectError,
  type ConnectOptions,
  connect,
  type Ending,
  type Frame,
  type WebSocketLike,
  type Welcome,
} from "../src/index.js";
import { CONTRACT, encodeError, framesOf, HB, IDLE, message, welcome } from "./support.js";

class FakeSocket implements WebSocketLike {
  static last: FakeSocket | undefined;
  static count = 0;
  binaryType = "blob";
  protocol = "";
  onopen: ((event: unknown) => void) | null = null;
  onmessage: ((event: { readonly data: unknown }) => void) | null = null;
  onclose: ((event: { readonly code: number }) => void) | null = null;
  onerror: ((event: unknown) => void) | null = null;
  readonly sent: Frame[] = [];
  closedWith: { code: number | undefined; reason: string | undefined } | undefined;

  constructor(
    readonly url: string,
    readonly protocols: string | string[],
  ) {
    FakeSocket.last = this;
    FakeSocket.count++;
  }

  send(data: Uint8Array): void {
    this.sent.push(...framesOf(data));
  }

  close(code?: number, reason?: string): void {
    this.closedWith = { code, reason };
  }

  open(protocol = "pavia.1"): void {
    this.protocol = protocol;
    this.onopen?.({});
  }

  receive(frame: Frame, stream = 0): void {
    const data = message(frame, stream);
    this.onmessage?.({ data: data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength) });
  }

  drop(code: number): void {
    this.onclose?.({ code });
  }
}

function options(extra: Partial<ConnectOptions> = {}): ConnectOptions {
  return { WebSocket: FakeSocket, now: () => Date.now(), warn: () => {}, ...extra };
}

function socket(): FakeSocket {
  return FakeSocket.last as FakeSocket;
}

async function established(extra: Partial<ConnectOptions> = {}, fields: Partial<Welcome> = {}) {
  const pending = connect("wss://example.test/rt", options(extra));
  socket().open();
  socket().receive(welcome(fields));
  return { connection: await pending, ws: socket() };
}

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
      ...message({ type: "END" }, 2),
      ...message({ type: "CLOSE", code: 0n }),
    ]);
    ws.onmessage?.({ data: both.buffer });
    const connection = await pending;
    const seen: string[] = [];
    for (const type of ["contractMismatch", "frame", "close"]) connection.addEventListener(type, () => seen.push(type));
    vi.advanceTimersByTime(0);
    expect(seen).toEqual(["contractMismatch", "frame", "close"]);
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

  it("passes frames of other streams on, and sends on the streams it opens", async () => {
    const { connection, ws } = await established();
    const frames = vi.fn();
    connection.addEventListener("frame", frames);
    vi.advanceTimersByTime(0);
    ws.receive({ type: "END" }, 2);
    expect((frames.mock.calls[0]?.[0] as CustomEvent).detail).toEqual({ stream: 2, frame: { type: "END" } });
    const stream = connection.openStream();
    connection.send(stream, { type: "CANCEL" });
    expect([stream, ws.sent.at(-1)]).toEqual([2, { type: "CANCEL" }]);
  });

  it("ends with PROTOCOL_ERROR on a text message", async () => {
    const { connection, ws } = await established();
    ws.onmessage?.({ data: "text" });
    expect(connection.closedWith).toMatchObject({ code: CloseCode.PROTOCOL_ERROR, by: "local" });
  });
});
