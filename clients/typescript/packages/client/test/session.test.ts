import { describe, expect, it } from "vitest";
import {
  ClientSession,
  CloseCode,
  closeReason,
  decodeFrame,
  decodeMessage,
  encodeMessages,
  type Frame,
  type SessionOptions,
  type SessionOutput,
  toRawFrame,
  type Welcome,
} from "../src/index.js";
import { encodeError } from "./support.js";

const HB = 50;
const IDLE = 200;
const CONTRACT = new Uint8Array(32).fill(7);

function welcome(fields: Partial<Welcome> = {}): Welcome {
  return {
    type: "WELCOME",
    version: 1n,
    codec: "cbor",
    caps: [],
    session: "s1",
    limits: { maxCalls: 100n, maxFrame: 1_048_576n, maxSubs: 256n, maxDgram: 1100n },
    hb: BigInt(HB),
    idle: BigInt(IDLE),
    ...fields,
  };
}

function message(frame: Frame, stream = 0): Uint8Array {
  return encodeMessages([{ stream, frame: toRawFrame(frame) }], 1 << 20)[0] as Uint8Array;
}

function drain(session: ClientSession): SessionOutput[] {
  const out: SessionOutput[] = [];
  for (let o = session.pollOutput(); o !== undefined; o = session.pollOutput()) out.push(o);
  return out;
}

function sent(outputs: readonly SessionOutput[]): Frame[] {
  return outputs.flatMap((o) =>
    o.kind === "transmit" ? decodeMessage(o.data, 1 << 20).map(({ frame }) => decodeFrame(frame)) : [],
  );
}

function kinds(outputs: readonly SessionOutput[]): string[] {
  return outputs.map((o) => (o.kind === "transmit" ? `transmit ${sent([o]).map((f) => f.type).join(",")}` : o.kind));
}

function established(options: SessionOptions = {}, fields: Partial<Welcome> = {}): ClientSession {
  const session = new ClientSession(options, 0);
  session.handleInput({ kind: "open" }, 0);
  drain(session);
  session.handleInput({ kind: "message", data: message(welcome(fields)) }, 10);
  drain(session);
  return session;
}

function closeOf(outputs: readonly SessionOutput[]): { code: number; reason: string } | undefined {
  const close = outputs.find((o) => o.kind === "close_transport");
  return close?.kind === "close_transport" ? { code: close.code, reason: close.reason } : undefined;
}

describe("client session", () => {
  it("sends HELLO when the transport opens and is established by WELCOME", () => {
    const session = new ClientSession({ codecs: ["cbor", "json"], contract: CONTRACT }, 0);
    expect(drain(session)).toEqual([]);
    session.handleInput({ kind: "open" }, 1);
    const [hello] = sent(drain(session));
    expect(hello).toMatchObject({ type: "HELLO", versions: [1n], codecs: ["cbor", "json"], client: { name: "pavia-ts" } });
    session.handleInput({ kind: "message", data: message(welcome()) }, 5);
    expect(kinds(drain(session))).toEqual(["established"]);
    expect(session.welcome?.session).toBe("s1");
    expect(session.pollTimeout()).toBe(5 + HB);
  });

  it("refuses options no server accepts before anything is sent", () => {
    expect(encodeError(() => new ClientSession({ limits: { maxFrame: 32_768n } }, 0)).kind).toBe("limits_below_floor");
    expect(encodeError(() => new ClientSession({ limits: { window: 1n } }, 0)).kind).toBe("limits_below_floor");
    expect(encodeError(() => new ClientSession({ contract: new Uint8Array(31) }, 0)).kind).toBe("invalid_field");
  });

  it("answers PING with PONG carrying the same bytes", () => {
    const session = established();
    session.handleInput({ kind: "message", data: message({ type: "PING", data: Uint8Array.of(1, 2, 3) }) }, 20);
    expect(sent(drain(session))).toEqual([{ type: "PONG", data: Uint8Array.of(1, 2, 3) }]);
  });

  it("sends PING after hb with nothing sent, and sending moves the next one", () => {
    const session = established();
    session.handleInput({ kind: "timeout" }, 10 + HB - 1);
    expect(drain(session)).toEqual([]);
    session.handleInput({ kind: "timeout" }, 10 + HB);
    expect(sent(drain(session))).toEqual([{ type: "PING", data: new Uint8Array(0) }]);
    expect(session.pollTimeout()).toBe(10 + 2 * HB);
    session.send(2, { type: "CANCEL" } as Frame, 10 + HB + 30);
    drain(session);
    expect(session.pollTimeout()).toBe(10 + HB + 30 + HB);
  });

  it("closes with IDLE_TIMEOUT when nothing arrives for idle, and a late message still counts", () => {
    const session = established({}, { hb: 10_000n });
    session.handleInput({ kind: "message", data: message({ type: "PONG", data: new Uint8Array(0) }) }, 10 + IDLE);
    expect(drain(session)).toEqual([]);
    session.handleInput({ kind: "timeout" }, 10 + IDLE);
    expect(drain(session)).toEqual([]);
    session.handleInput({ kind: "timeout" }, 10 + 2 * IDLE);
    const out = drain(session);
    expect(sent(out)).toEqual([{ type: "CLOSE", code: BigInt(CloseCode.IDLE_TIMEOUT) }]);
    expect(closeOf(out)).toEqual({ code: 4000 + CloseCode.IDLE_TIMEOUT, reason: "" });
    expect(out.at(-1)).toEqual({ kind: "ended", ending: { kind: "closed", code: CloseCode.IDLE_TIMEOUT, by: "local" } });
    expect(session.pollTimeout()).toBeUndefined();
  });

  it("closes with HANDSHAKE_TIMEOUT when WELCOME does not come in time", () => {
    const opened = new ClientSession({ handshakeTimeout: 100 }, 0);
    opened.handleInput({ kind: "open" }, 0);
    drain(opened);
    expect(opened.pollTimeout()).toBe(100);
    opened.handleInput({ kind: "message", data: message(welcome()) }, 100);
    const out = drain(opened);
    expect(sent(out).map((f) => f.type)).toEqual(["CLOSE"]);
    expect(closeOf(out)?.code).toBe(4000 + CloseCode.HANDSHAKE_TIMEOUT);
    const connecting = new ClientSession({ handshakeTimeout: 100 }, 0);
    connecting.handleInput({ kind: "timeout" }, 100);
    expect(kinds(drain(connecting))).toEqual(["close_transport", "ended"]);
  });

  it("ends on REJECT with its code, message and retry", () => {
    const session = new ClientSession({}, 0);
    session.handleInput({ kind: "open" }, 0);
    drain(session);
    const reject: Frame = { type: "REJECT", code: BigInt(CloseCode.UNAUTHENTICATED), msg: "no", retry: 500n };
    session.handleInput({ kind: "message", data: message(reject) }, 1);
    const out = drain(session);
    expect(closeOf(out)).toEqual({ code: 4000 + CloseCode.UNAUTHENTICATED, reason: "no" });
    expect(out.at(-1)).toEqual({
      kind: "ended",
      ending: { kind: "rejected", code: CloseCode.UNAUTHENTICATED, msg: "no", retry: 500n },
    });
  });

  it("closes with PROTOCOL_ERROR on a text message, a truncated WsFrame or an oversized message", () => {
    const truncated = message({ type: "PING", data: Uint8Array.of(1) }).subarray(0, 4);
    for (const data of ["hello", truncated, new Uint8Array(2 * 1_048_576 + 1)]) {
      const session = established();
      session.handleInput({ kind: "message", data }, 20);
      const out = drain(session);
      expect(closeOf(out)?.code, typeof data === "string" ? data : String(data.length)).toBe(4000 + CloseCode.PROTOCOL_ERROR);
    }
  });

  it("closes with FRAME_TOO_LARGE for a frame above this side's max_frame", () => {
    const session = established({ limits: { maxFrame: 65_536n } });
    const big = encodeMessages([{ stream: 2, frame: toRawFrame({ type: "ITEM", data: new Uint8Array(70_000) } as Frame) }], 1 << 20);
    session.handleInput({ kind: "message", data: big[0] as Uint8Array }, 20);
    expect(closeOf(drain(session))?.code).toBe(4000 + CloseCode.FRAME_TOO_LARGE);
  });

  it("ends on the peer's CLOSE, an unknown code read as INTERNAL", () => {
    const session = established();
    session.handleInput({ kind: "message", data: message({ type: "CLOSE", code: 99n, msg: "bye" }) }, 20);
    const out = drain(session);
    expect(closeOf(out)).toEqual({ code: 4000 + CloseCode.INTERNAL, reason: "bye" });
    expect(out.at(-1)).toEqual({ kind: "ended", ending: { kind: "closed", code: CloseCode.INTERNAL, msg: "bye", by: "peer" } });
    session.handleInput({ kind: "message", data: message({ type: "PING", data: new Uint8Array(0) }) }, 30);
    expect(drain(session)).toEqual([]);
  });

  it("reads a WebSocket close code of 4000 + code, and any other close as a lost transport", () => {
    const going = established();
    going.handleInput({ kind: "transport_closed", code: 4000 + CloseCode.GOING_AWAY }, 20);
    expect(drain(going)).toEqual([{ kind: "ended", ending: { kind: "closed", code: CloseCode.GOING_AWAY, by: "peer" } }]);
    const unknown = established();
    unknown.handleInput({ kind: "transport_closed", code: 4999 }, 20);
    expect(drain(unknown)).toEqual([{ kind: "ended", ending: { kind: "closed", code: CloseCode.INTERNAL, by: "peer" } }]);
    const lost = established();
    lost.handleInput({ kind: "transport_closed", code: 1006 }, 20);
    expect(drain(lost)).toEqual([{ kind: "ended", ending: { kind: "transport_lost" } }]);
  });

  it("closes with CLOSE and 4000 + code, the reason cut to 123 bytes on a character boundary", () => {
    const session = established();
    const msg = `${"a".repeat(121)}é€`;
    session.close(CloseCode.GOING_AWAY, msg, 20);
    const out = drain(session);
    expect(sent(out)).toEqual([{ type: "CLOSE", code: BigInt(CloseCode.GOING_AWAY), msg }]);
    expect(closeOf(out)).toEqual({ code: 4000 + CloseCode.GOING_AWAY, reason: `${"a".repeat(121)}é` });
    expect(new TextEncoder().encode(closeReason("€".repeat(50))).length).toBe(123);
    expect(closeReason("\ud800")).toBe("");
  });

  it("drops frames still queued when it closes, and sends CLOSE first", () => {
    const session = established();
    session.send(2, { type: "CANCEL" } as Frame, 20);
    session.close(CloseCode.NORMAL, undefined, 20);
    expect(sent(drain(session)).map((f) => f.type)).toEqual(["CLOSE"]);
  });

  it("warns on a contract mismatch only when both fingerprints are known and differ", () => {
    const other = new Uint8Array(32).fill(9);
    const cases: [Uint8Array | undefined, Uint8Array | undefined, boolean][] = [
      [CONTRACT, other, true],
      [CONTRACT, CONTRACT, false],
      [CONTRACT, undefined, false],
      [undefined, other, false],
    ];
    for (const [local, remote, mismatch] of cases) {
      const session = new ClientSession(local === undefined ? {} : { contract: local }, 0);
      session.handleInput({ kind: "open" }, 0);
      drain(session);
      session.handleInput({ kind: "message", data: message(welcome(remote === undefined ? {} : { contract: remote })) }, 1);
      const out = drain(session);
      expect(kinds(out)).toEqual(mismatch ? ["contract_mismatch", "established"] : ["established"]);
    }
  });

  it("closes with PROTOCOL_ERROR on a WELCOME it cannot use or a frame out of place", () => {
    const bad: [Frame, number][] = [
      [welcome({ version: 2n }), 0],
      [welcome({ codec: "json" }), 0],
      [{ type: "PING", data: new Uint8Array(0) }, 0],
      [welcome(), 2],
    ];
    for (const [frame, stream] of bad) {
      const session = new ClientSession({}, 0);
      session.handleInput({ kind: "open" }, 0);
      drain(session);
      session.handleInput({ kind: "message", data: message(frame, stream) }, 1);
      expect(closeOf(drain(session))?.code).toBe(4000 + CloseCode.PROTOCOL_ERROR);
    }
    const again = established();
    again.handleInput({ kind: "message", data: message(welcome()) }, 20);
    expect(closeOf(drain(again))?.code).toBe(4000 + CloseCode.PROTOCOL_ERROR);
  });

  it("hands other frames to the call layer", () => {
    const session = established();
    const end: Frame = { type: "END" } as Frame;
    session.handleInput({ kind: "message", data: message(end, 2) }, 20);
    expect(drain(session)).toEqual([{ kind: "frame", stream: 2, frame: decodeFrame(toRawFrame(end)) }]);
  });

  it("allocates even stream IDs from 2 and sends only once established", () => {
    const session = new ClientSession({}, 0);
    expect([session.openStream(), session.openStream(), session.openStream()]).toEqual([2, 4, 6]);
    expect(encodeError(() => session.send(2, { type: "CANCEL" } as Frame, 0)).kind).toBe("invalid_value");
  });

  it("holds sent frames to the server's max_frame, read as at most 2^53 - 1, and to 16 KiB of header", () => {
    const small = established({}, { limits: { maxCalls: 1n, maxFrame: 65_536n, maxSubs: 1n, maxDgram: 1n } });
    const big = { type: "ITEM", data: new Uint8Array(70_000) } as Frame;
    expect(encodeError(() => small.send(2, big, 20)).kind).toBe("frame_too_large");
    const huge = established({}, { limits: { maxCalls: 1n, maxFrame: 2n ** 53n + 1n, maxSubs: 1n, maxDgram: 1n } });
    huge.send(2, big, 20);
    expect(sent(drain(huge))).toHaveLength(1);
    const meta = new Map([["k", "x".repeat(17_000)]]);
    expect(encodeError(() => huge.send(2, { type: "END", meta } as Frame, 20)).kind).toBe("frame_too_large");
  });
});
