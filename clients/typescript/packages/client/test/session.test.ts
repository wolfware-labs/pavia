import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  CLIENT_INFO,
  ClientSession,
  CloseCode,
  closeReason,
  decodeFrame,
  type Frame,
  type SessionOptions,
  type SessionOutput,
  toRawFrame,
  type Welcome,
} from "../src/index.js";
import { concat, CONTRACT, encodeError, framesOf, HB, IDLE, message, welcome } from "./support.js";

function drain(session: ClientSession): SessionOutput[] {
  const out: SessionOutput[] = [];
  for (let o = session.pollOutput(); o !== undefined; o = session.pollOutput()) out.push(o);
  return out;
}

function sent(outputs: readonly SessionOutput[]): Frame[] {
  return outputs.flatMap((o) =>
    o.kind === "transmit" ? framesOf(o.data) : [],
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

const RESUME_FIELDS = { token: new Uint8Array(16).fill(1), resumeWindow: 30_000n };

/** WELCOME fields granting `caps`, with the fields spec 7.2 asks for when `resume` is among them. */
function grant(caps: readonly string[]): Partial<Welcome> {
  return { caps: [...caps], ...(caps.includes("resume") ? RESUME_FIELDS : {}) };
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
    expect(hello).toMatchObject({
      type: "HELLO",
      versions: [1n],
      codecs: ["cbor", "json"],
      client: { name: "pavia-ts" },
    });
    session.handleInput({ kind: "message", data: message(welcome()) }, 5);
    expect(kinds(drain(session))).toEqual(["established"]);
    expect(session.welcome?.session).toBe("s1");
    expect(session.pollTimeout()).toBe(5 + HB);
  });

  it("names this package and its version in HELLO", () => {
    const text = readFileSync(new URL("../package.json", import.meta.url), "utf8");
    const manifest = JSON.parse(text) as { version: string };
    expect(CLIENT_INFO).toEqual({ name: "pavia-ts", version: manifest.version });
  });

  it("refuses a message that arrives before the transport is open", () => {
    const session = new ClientSession({}, 0);
    session.handleInput({ kind: "message", data: message(welcome()) }, 1);
    const out = drain(session);
    expect(sent(out)).toEqual([]);
    expect(closeOf(out)?.code).toBe(4000 + CloseCode.PROTOCOL_ERROR);
    expect(session.welcome).toBeUndefined();
    session.handleInput({ kind: "open" }, 2);
    expect(drain(session)).toEqual([]);
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
    expect(out.at(-1)).toEqual({
      kind: "ended",
      ending: { kind: "closed", code: CloseCode.IDLE_TIMEOUT, by: "local" },
    });
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
      const label = typeof data === "string" ? data : String(data.length);
      expect(closeOf(out)?.code, label).toBe(4000 + CloseCode.PROTOCOL_ERROR);
    }
  });

  it("closes with FRAME_TOO_LARGE for a frame above this side's max_frame", () => {
    const session = established({ limits: { maxFrame: 65_536n } });
    const big = message({ type: "ITEM", data: new Uint8Array(70_000) } as Frame, 2);
    session.handleInput({ kind: "message", data: big }, 20);
    expect(closeOf(drain(session))?.code).toBe(4000 + CloseCode.FRAME_TOO_LARGE);
  });

  it("ends on the peer's CLOSE, an unknown code read as INTERNAL", () => {
    const session = established();
    session.handleInput({ kind: "message", data: message({ type: "CLOSE", code: 99n, msg: "bye" }) }, 20);
    const out = drain(session);
    expect(closeOf(out)).toEqual({ code: 4000 + CloseCode.INTERNAL, reason: "bye" });
    expect(out.at(-1)).toEqual({
      kind: "ended",
      ending: { kind: "closed", code: CloseCode.INTERNAL, msg: "bye", by: "peer" },
    });
    session.handleInput({ kind: "message", data: message({ type: "PING", data: new Uint8Array(0) }) }, 30);
    expect(drain(session)).toEqual([]);
  });

  it("reads a transport that closes without CLOSE as lost, with its close code (spec 7.7)", () => {
    for (const code of [4000 + CloseCode.GOING_AWAY, 1006, undefined]) {
      const session = established();
      session.handleInput({ kind: "transport_closed", code }, 20);
      expect(drain(session)).toEqual([{ kind: "ended", ending: { kind: "transport_lost", code } }]);
    }
  });

  it("closes with CLOSE and 4000 + code, the reason cut to 123 bytes on a character boundary", () => {
    const session = established();
    const msg = `${"a".repeat(121)}é€`;
    session.close(CloseCode.GOING_AWAY, msg, 20);
    const out = drain(session);
    const cut = `${"a".repeat(121)}é`;
    expect(sent(out)).toEqual([{ type: "CLOSE", code: BigInt(CloseCode.GOING_AWAY), msg: cut }]);
    expect(closeOf(out)).toEqual({ code: 4000 + CloseCode.GOING_AWAY, reason: cut });
    const ending = { kind: "closed", code: CloseCode.GOING_AWAY, msg: cut, by: "local" };
    expect(out.at(-1)).toEqual({ kind: "ended", ending });
    expect(new TextEncoder().encode(closeReason("€".repeat(50))).length).toBe(123);
    expect(closeReason("\ud800")).toBe("");
    expect(closeReason(`\ufeff${"a".repeat(200)}`)).toBe(`\ufeff${"a".repeat(120)}`);
  });

  it("sends a close code that is not a CloseCode as INTERNAL", () => {
    const session = established();
    session.close(1.5 as CloseCode, undefined, 20);
    const out = drain(session);
    expect(session.ended).toBe(true);
    expect(closeOf(out)?.code).toBe(4000 + CloseCode.INTERNAL);
  });

  it("ends even with a close message that cannot be sent as it is", () => {
    for (const [msg, opened] of [["\ud800", true], ["x".repeat(20_000), true], ["y".repeat(70_000), false]] as const) {
      const session = opened ? established() : new ClientSession({}, 0);
      if (!opened) {
        session.handleInput({ kind: "open" }, 0);
        drain(session);
      }
      session.close(CloseCode.NORMAL, msg, 20);
      const out = drain(session);
      expect(session.ended).toBe(true);
      const close = sent(out).find((frame) => frame.type === "CLOSE") as { msg?: string } | undefined;
      expect(new TextEncoder().encode(close?.msg ?? "").length).toBeLessThanOrEqual(123);
    }
  });

  it("refuses a HELLO a server cannot read before WELCOME and a handshake timeout that is no number", () => {
    const token = (length: number) => ({ scheme: "bearer", token: "t".repeat(length) });
    for (const length of [20_000, 70_000]) {
      const refused = encodeError(() => new ClientSession({ auth: token(length) } as SessionOptions, 0));
      expect(refused.kind).toBe("frame_too_large");
    }
    expect(() => new ClientSession({ auth: token(10_000) } as SessionOptions, 0)).not.toThrow();
    for (const handshakeTimeout of [0, -1, Number.NaN, Number.POSITIVE_INFINITY]) {
      expect(() => new ClientSession({ handshakeTimeout }, 0)).toThrow(RangeError);
    }
  });

  it("closes with PROTOCOL_ERROR on a frame on the wrong kind of stream or in the wrong direction", () => {
    const cases: [Frame, number][] = [
      [{ type: "CLOSE", code: 0n }, 2],
      [{ type: "PING", data: new Uint8Array(0) }, 4],
      [welcome(), 2],
      [{ type: "CANCEL" }, 0],
      [{ type: "END" }, 0],
      [{ type: "SUBSCRIBE", rid: 1n, channel: "c" } as Frame, 0],
      [{ type: "UNSUBSCRIBE", rid: 1n, channel: "c" } as Frame, 0],
    ];
    for (const [frame, stream] of cases) {
      const session = established();
      session.handleInput({ kind: "message", data: message(frame, stream) }, 20);
      expect(closeOf(drain(session))?.code, `${frame.type} on ${stream}`).toBe(4000 + CloseCode.PROTOCOL_ERROR);
    }
  });

  it("closes with PROTOCOL_ERROR on a WELCOME granting a capability HELLO did not request (spec 7.10)", () => {
    for (const [requested, granted, ok] of [
      [undefined, ["server-calls"], false],
      [["channels"], ["channels", "resume"], false],
      [["channels", "resume"], ["resume"], true],
      [undefined, ["calls"], true],
    ] as const) {
      const session = new ClientSession(requested === undefined ? {} : { caps: requested }, 0);
      session.handleInput({ kind: "open" }, 0);
      drain(session);
      session.handleInput({ kind: "message", data: message(welcome(grant(granted))) }, 1);
      const expected = ok ? ["established"] : ["transmit CLOSE", "close_transport", "ended"];
      expect(kinds(drain(session)), granted.join(",")).toEqual(expected);
    }
  });

  it("closes with PROTOCOL_ERROR on a frame that needs a capability WELCOME did not grant", () => {
    const publication: Frame = { type: "PUBLICATION", channel: "c", offset: 1n, data: new Uint8Array(0) };
    const call: Frame = { type: "CALL", method: "m", shape: 0n, data: new Uint8Array(0) };
    const sequenced: Frame = { type: "NOTIFY", method: "m", seq: 1n, data: new Uint8Array(0) };
    const refused: [Frame, number, string[]][] = [
      [publication, 0, []],
      [{ type: "DATAGRAM", key: 1n, seq: 1n, data: new Uint8Array(0) }, 0, ["channels"]],
      [call, 3, []],
      [{ ...call, shape: 1n }, 3, ["server-calls"]],
      [sequenced, 0, ["channels"]],
    ];
    for (const [frame, stream, caps] of refused) {
      const session = established({ caps }, grant(caps));
      session.handleInput({ kind: "message", data: message(frame, stream) }, 20);
      const what = `${frame.type} with ${caps.join(",")}`;
      expect(closeOf(drain(session))?.code, what).toBe(4000 + CloseCode.PROTOCOL_ERROR);
    }
    const allowed: [Frame, number, string[]][] = [
      [publication, 0, ["channels"]],
      [call, 3, ["server-calls"]],
      [{ type: "NOTIFY", method: "m", data: new Uint8Array(0) }, 0, []],
      [sequenced, 0, ["resume"]],
    ];
    for (const [frame, stream, caps] of allowed) {
      const session = established({ caps }, grant(caps));
      session.handleInput({ kind: "message", data: message(frame, stream) }, 20);
      expect(kinds(drain(session)), `${frame.type} with ${caps.join(",")}`).toEqual(["frame"]);
    }
  });

  it("closes with PROTOCOL_ERROR on a WELCOME whose limits leave out window (spec 7.2, WebSocket)", () => {
    const session = new ClientSession({}, 0);
    session.handleInput({ kind: "open" }, 0);
    drain(session);
    const limits = { maxCalls: 1n, maxFrame: 65_536n, maxSubs: 1n, maxDgram: 1n };
    session.handleInput({ kind: "message", data: message(welcome({ limits })) }, 1);
    expect(closeOf(drain(session))?.code).toBe(4000 + CloseCode.PROTOCOL_ERROR);
  });

  it("closes with PROTOCOL_ERROR on a session ID outside spec 7.2's form", () => {
    const ids: [string, boolean][] = [
      ["", false],
      ["a b", false],
      ["a/b", false],
      ["x".repeat(65), false],
      ["x".repeat(64), true],
      ["A-z_9", true],
    ];
    for (const [id, ok] of ids) {
      const session = new ClientSession({}, 0);
      session.handleInput({ kind: "open" }, 0);
      drain(session);
      session.handleInput({ kind: "message", data: message(welcome({ session: id })) }, 1);
      expect(kinds(drain(session))[0], JSON.stringify(id)).toBe(ok ? "established" : "transmit CLOSE");
    }
  });

  it("closes with PROTOCOL_ERROR on resume granted without token and resume_window", () => {
    const token = Uint8Array.of(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16);
    const cases: [Partial<Welcome>, boolean][] = [
      [{ caps: ["resume"] }, false],
      [{ caps: ["resume"], token }, false],
      [{ caps: ["resume"], resumeWindow: 30_000n }, false],
      [{ caps: ["resume"], token, resumeWindow: 30_000n }, true],
    ];
    for (const [fields, ok] of cases) {
      const session = new ClientSession({ caps: ["resume"] }, 0);
      session.handleInput({ kind: "open" }, 0);
      drain(session);
      session.handleInput({ kind: "message", data: message(welcome(fields)) }, 1);
      expect(kinds(drain(session))[0], JSON.stringify(Object.keys(fields))).toBe(ok ? "established" : "transmit CLOSE");
    }
  });

  it("closes with PROTOCOL_ERROR on a WELCOME with hb or idle 0", () => {
    for (const fields of [{ hb: 0n }, { idle: 0n }]) {
      const session = new ClientSession({}, 0);
      session.handleInput({ kind: "open" }, 0);
      drain(session);
      session.handleInput({ kind: "message", data: message(welcome(fields)) }, 1);
      expect(closeOf(drain(session))?.code).toBe(4000 + CloseCode.PROTOCOL_ERROR);
    }
  });

  it("cuts a close reason to the longest valid prefix when the limit falls inside a character", () => {
    for (const char of ["\u00e9", "\u20ac", "\u{1F600}"]) {
      for (const ascii of [119, 120, 121, 122]) {
        const msg = `${"a".repeat(ascii)}${char}b`;
        const reason = closeReason(msg);
        const bytes = new TextEncoder().encode(reason).length;
        const fits = ascii + new TextEncoder().encode(char).length <= 123;
        expect([bytes, msg.startsWith(reason)], `${ascii} + ${char}`).toEqual([fits ? bytes : ascii, true]);
        expect(reason.length).toBeGreaterThan(0);
        if (!fits) expect(reason).toBe("a".repeat(ascii));
      }
    }
  });

  it("reports a WELCOME read in the message whose next frame closes the session", () => {
    const session = new ClientSession({ contract: CONTRACT }, 0);
    session.handleInput({ kind: "open" }, 0);
    drain(session);
    const data = concat([message(welcome({ contract: new Uint8Array(32).fill(9) })), message({ type: "END" }, 0)]);
    session.handleInput({ kind: "message", data }, 1);
    const out = drain(session);
    expect(kinds(out)).toEqual(["contract_mismatch", "established", "transmit CLOSE", "close_transport", "ended"]);
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
      const fields = remote === undefined ? {} : { contract: remote };
      session.handleInput({ kind: "message", data: message(welcome(fields)) }, 1);
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

  it("passes LANE_OPEN on a lane stream to the call layer when lanes are granted", () => {
    const session = established({ caps: ["lanes"] }, { caps: ["lanes"] });
    session.handleInput({ kind: "message", data: message({ type: "LANE_OPEN", lane: 1n }, 3) }, 20);
    expect(kinds(drain(session))).toEqual(["frame"]);
  });

  it("ignores extension frames on any stream", () => {
    for (const stream of [0, 2, 3]) {
      const session = established();
      const header = new Uint8Array(0);
      const extension: Frame = { type: "EXTENSION", frameType: 0x80, header, data: Uint8Array.of(1) };
      session.handleInput({ kind: "message", data: message(extension, stream) }, 20);
      expect(drain(session), String(stream)).toEqual([]);
    }
  });

  it("checks WELCOME against the HELLO it sent, whatever the caller changes later", () => {
    const codecs = ["cbor"] as ("cbor" | "json")[];
    const caps = ["channels"];
    const session = new ClientSession({ codecs, caps }, 0);
    codecs.push("json");
    caps.push("resume");
    session.handleInput({ kind: "open" }, 0);
    const [hello] = sent(drain(session));
    expect(hello).toMatchObject({ codecs: ["cbor"], caps: ["channels"] });
    session.handleInput({ kind: "message", data: message(welcome({ codec: "json" })) }, 1);
    expect(closeOf(drain(session))?.code).toBe(4000 + CloseCode.PROTOCOL_ERROR);
  });

  it("hands other frames to the call layer", () => {
    const session = established();
    const end: Frame = { type: "END" } as Frame;
    session.handleInput({ kind: "message", data: message(end, 2) }, 20);
    expect(drain(session)).toEqual([{ kind: "frame", stream: 2, frame: decodeFrame(toRawFrame(end)) }]);
  });

  it("opens even stream IDs from 2, takes none for a refused frame, and sends only once established", () => {
    const cancel = { type: "CANCEL" } as Frame;
    expect(encodeError(() => new ClientSession({}, 0).open(cancel, 0)).kind).toBe("invalid_value");
    expect(encodeError(() => new ClientSession({}, 0).send(2, cancel, 0)).kind).toBe("invalid_value");
    const session = established();
    const big = { type: "CANCEL", msg: "x".repeat(17_000) } as Frame;
    expect(encodeError(() => session.open(big, 20)).kind).toBe("frame_too_large");
    expect([session.open(cancel, 20), session.open(cancel, 20), session.open(cancel, 20)]).toEqual([2, 4, 6]);
    expect(session.lastStream).toBe(6);
  });

  it("holds sent frames to the server's max_frame, read as at most 2^53 - 1, and to 16 KiB of header", () => {
    const limits = { maxCalls: 1n, maxSubs: 1n, maxDgram: 1n, window: 65_536n };
    const small = established({}, { limits: { ...limits, maxFrame: 65_536n } });
    const big = { type: "ITEM", data: new Uint8Array(70_000) } as Frame;
    expect(encodeError(() => small.send(2, big, 20)).kind).toBe("frame_too_large");
    const huge = established({}, { limits: { ...limits, maxFrame: 2n ** 53n + 1n } });
    huge.send(2, big, 20);
    expect(sent(drain(huge))).toHaveLength(1);
    const meta = new Map([["k", "x".repeat(17_000)]]);
    expect(encodeError(() => huge.send(2, { type: "END", meta } as Frame, 20)).kind).toBe("frame_too_large");
  });
});
