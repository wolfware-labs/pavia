import { describe, expect, it } from "vitest";
import {
  CloseCode,
  closeCodeFromWire,
  decodeFrame,
  decodeHeader,
  decodeMessage,
  decodeVarint,
  encodeFrame,
  encodeHeader,
  encodeMessages,
  encodeRawFrame,
  encodeVarint,
  type Frame,
  type HeaderMap,
  type HeaderValue,
  MAX_FRAME_FLOOR,
  MAX_VARINT,
  type RawFrame,
  StreamFrameDecoder,
  takeFrame,
  toRawFrame,
} from "../src/index.js";
import { concat, encodeError, hexToBytes, protocolError } from "./support.js";

const u = (n: number | bigint): HeaderValue => ({ kind: "uint", value: BigInt(n) });
const t = (value: string): HeaderValue => ({ kind: "text", value });

function raw(type: number, header: HeaderMap | undefined, data: number[] = []): RawFrame {
  return { type, flags: 0, header: header ? encodeHeader(header) : new Uint8Array(0), data: Uint8Array.from(data) };
}

describe("varints", () => {
  it("reads a value above 2^32 without truncating it", () => {
    const bytes = encodeVarint(2n ** 32n + 5n);
    expect(decodeVarint(bytes)).toEqual({ kind: "value", value: 2n ** 32n + 5n, length: 8 });
  });

  it("keeps values above 2^53 - 1 exact", () => {
    for (const value of [2n ** 53n, 2n ** 53n + 1n, MAX_VARINT]) {
      expect(decodeVarint(encodeVarint(value))).toEqual({ kind: "value", value, length: 8 });
    }
  });

  it("refuses a value above the caller's limit without reading further", () => {
    expect(decodeVarint(hexToBytes("80 00 40 00 ff"), 0, 16_383n)).toEqual({ kind: "above_max", value: 16_384n, max: 16_383n });
  });

  it("refuses numbers that are not safe integers", () => {
    for (const value of [-1, 1.5, 2 ** 53, Number.NaN]) {
      expect(encodeError(() => encodeVarint(value)).kind).toBe("out_of_range");
    }
    expect(encodeError(() => encodeVarint(-1n)).kind).toBe("out_of_range");
  });
});

describe("frames on the wire", () => {
  const huge = hexToBytes("04 00 c0 00 00 01 00 00 00 05");

  it("refuses a Length of 2^32 + 5 against max_frame", () => {
    expect(protocolError(() => takeFrame(huge, 0, MAX_FRAME_FLOOR)).outcome).toEqual({
      action: "close",
      code: CloseCode.FRAME_TOO_LARGE,
    });
    expect(protocolError(() => decodeMessage(concat([Uint8Array.of(0), huge]), MAX_FRAME_FLOOR)).kind).toBe("frame_too_large");
  });

  it("waits for a Length of 2^32 + 5 under a huge limit instead of reading it as 5", () => {
    expect(takeFrame(huge, 0, Number.MAX_SAFE_INTEGER)).toBeUndefined();
    const decoder = new StreamFrameDecoder(Number.MAX_SAFE_INTEGER);
    decoder.push(huge);
    expect(decoder.next()).toBeUndefined();
    expect(protocolError(() => decoder.finish()).kind).toBe("stream_ended_mid_frame");
  });

  it("refuses a HeaderLength of 2^32 + 5 past the payload", () => {
    const bytes = hexToBytes("01 00 09 c0 00 00 01 00 00 00 05 a0");
    expect(protocolError(() => takeFrame(bytes, 0, MAX_FRAME_FLOOR)).kind).toBe("malformed_header");
  });

  it("refuses a frame Length above max_frame before buffering the payload", () => {
    const decoder = new StreamFrameDecoder(100);
    expect(protocolError(() => decoder.push(hexToBytes("10 00 40 65 00"))).kind).toBe("frame_too_large");
  });

  it("decodes frames fed one byte at a time", () => {
    const frames = [raw(0x04, undefined, [1, 2]), raw(0x10, [[u(2), t("m")], [u(5), u(0)]], [0xf6])];
    const bytes = Uint8Array.from(frames.flatMap((f) => [...encodeRawFrame(f)]));
    const decoder = new StreamFrameDecoder(MAX_FRAME_FLOOR);
    const out: RawFrame[] = [];
    for (const byte of bytes) {
      decoder.push(Uint8Array.of(byte));
      for (let f = decoder.next(); f !== undefined; f = decoder.next()) out.push(f);
    }
    decoder.finish();
    expect(out).toEqual(frames);
  });

  it("reports a stream cut at every offset", () => {
    const bytes = encodeRawFrame(raw(0x10, [[u(2), t("m")], [u(5), u(0)]], [0xf6]));
    for (let cut = 1; cut < bytes.length; cut++) {
      const decoder = new StreamFrameDecoder(MAX_FRAME_FLOOR);
      decoder.push(bytes.subarray(0, cut));
      expect(decoder.next()).toBeUndefined();
      expect(protocolError(() => decoder.finish()).kind).toBe("stream_ended_mid_frame");
    }
  });

  it("keeps the frames it returned when the buffer grows", () => {
    const decoder = new StreamFrameDecoder(MAX_FRAME_FLOOR);
    decoder.push(encodeRawFrame(raw(0x04, undefined, [1, 2, 3])));
    const first = decoder.next();
    decoder.push(encodeRawFrame(raw(0x04, undefined, new Array<number>(500).fill(9))));
    expect([...(first?.data ?? [])]).toEqual([1, 2, 3]);
    expect(decoder.next()?.data.length).toBe(500);
  });
});

describe("WebSocket messages", () => {
  it("refuses text, empty and oversized messages", () => {
    expect(protocolError(() => decodeMessage("hi", MAX_FRAME_FLOOR)).kind).toBe("text_message");
    expect(protocolError(() => decodeMessage(new Uint8Array(0), MAX_FRAME_FLOOR)).kind).toBe("malformed_message");
    const oversized = new Uint8Array(2 * 100 + 1);
    expect(protocolError(() => decodeMessage(oversized, 100)).kind).toBe("message_too_large");
  });

  it("accepts a message of exactly 2 x max_frame", () => {
    const frames = [65_527, 65_529].map((length, i) => ({
      stream: 2 + 2 * i,
      frame: raw(0x11, undefined, new Array<number>(length).fill(i)),
    }));
    const messages = encodeMessages(frames);
    expect(messages.map((m) => m.length)).toEqual([2 * MAX_FRAME_FLOOR]);
    expect(decodeMessage(messages[0] as Uint8Array, MAX_FRAME_FLOOR)).toEqual(frames);
    const longer = Uint8Array.from([...(messages[0] as Uint8Array), 0]);
    expect(protocolError(() => decodeMessage(longer, MAX_FRAME_FLOOR)).kind).toBe("message_too_large");
  });

  it("refuses a partial WsFrame after the last complete one", () => {
    const message = encodeMessages([{ stream: 0, frame: raw(0x04, undefined) }])[0] as Uint8Array;
    for (const extra of [[0x00], [0x00, 0x04], [0x00, 0x04, 0x00], [0x40]]) {
      expect(protocolError(() => decodeMessage(Uint8Array.from([...message, ...extra]), MAX_FRAME_FLOOR)).kind).toBe(
        "leftover_bytes",
      );
    }
  });

  it("refuses stream IDs above 2^53 - 1", () => {
    const frame = encodeRawFrame(raw(0x04, undefined));
    const stream = encodeVarint(2n ** 53n);
    expect(protocolError(() => decodeMessage(Uint8Array.from([...stream, ...frame]), MAX_FRAME_FLOOR)).outcome).toEqual({
      action: "close",
      code: CloseCode.PROTOCOL_ERROR,
    });
    const safe = encodeVarint(BigInt(Number.MAX_SAFE_INTEGER));
    expect(decodeMessage(Uint8Array.from([...safe, ...frame]), MAX_FRAME_FLOOR)[0]?.stream).toBe(Number.MAX_SAFE_INTEGER);
    expect(encodeError(() => encodeMessages([{ stream: 2 ** 53, frame: raw(0x04, undefined) }])).kind).toBe(
      "stream_id_out_of_range",
    );
  });

  it("refuses a raw type or flags that is not a byte", () => {
    for (const frame of [
      { ...raw(0x04, undefined), type: 0x101 },
      { ...raw(0x04, undefined), type: -1 },
      { ...raw(0x04, undefined), type: 1.5 },
      { ...raw(0x04, undefined), flags: 0x100 },
    ]) {
      expect(encodeError(() => encodeRawFrame(frame)).kind).toBe("out_of_range");
      expect(encodeError(() => encodeMessages([{ stream: 0, frame }])).kind).toBe("out_of_range");
    }
    expect([...encodeRawFrame({ ...raw(0xff, undefined), flags: 0xff })].slice(0, 2)).toEqual([0xff, 0xff]);
  });

  it("batches frames and splits above 2 x max_frame", () => {
    const small = encodeMessages([
      { stream: 0, frame: raw(0x04, undefined) },
      { stream: 2, frame: raw(0x15, undefined) },
    ]);
    expect(small.map((m) => [...m])).toEqual([[...hexToBytes("00 04 00 01 00 02 15 00 01 00")]]);
    const big = raw(0x11, undefined, new Array<number>(60_000).fill(7));
    const frames = [2, 4, 6].map((stream) => ({ stream, frame: big }));
    const messages = encodeMessages(frames);
    expect(messages.length).toBe(2);
    for (const m of messages) expect(m.length).toBeLessThanOrEqual(2 * MAX_FRAME_FLOOR);
    expect(messages.flatMap((m) => decodeMessage(m, MAX_FRAME_FLOOR))).toEqual(frames);
    expect(encodeError(() => encodeMessages(frames, 100)).kind).toBe("frame_too_large");
  });
});

describe("header sections", () => {
  it("refuses a byte string declaring 2^32 + 5 bytes", () => {
    expect(protocolError(() => decodeHeader(hexToBytes("a1 01 5b 00 00 00 01 00 00 00 05 00"))).reason).toBe("malformed");
  });

  it("refuses an array declaring 2^32 items", () => {
    expect(protocolError(() => decodeHeader(hexToBytes("a1 01 9a 00 01 00 00 00 00"))).reason).toBe("malformed");
  });

  it("keeps 64-bit integers exact", () => {
    const header: HeaderMap = [
      [u(1), u(2n ** 64n - 1n)],
      [u(2), { kind: "nint", value: 2n ** 64n - 1n }],
    ];
    expect(decodeHeader(encodeHeader(header))).toEqual(header);
  });

  it("refuses invalid UTF-8 and keeps a byte order mark", () => {
    expect(protocolError(() => decodeHeader(hexToBytes("a1 01 62 c3 28"))).reason).toBe("malformed");
    expect(decodeHeader(hexToBytes("a1 01 63 ef bb bf"))).toEqual([[u(1), t("﻿")]]);
  });

  it("writes floats in the shortest exact form", () => {
    const cases: [number, string][] = [
      [0, "f9 00 00"],
      [-0, "f9 80 00"],
      [1.5, "f9 3e 00"],
      [65504, "f9 7b ff"],
      [5.960464477539063e-8, "f9 00 01"],
      [Infinity, "f9 7c 00"],
      [Number.NaN, "f9 7e 00"],
      [100000, "fa 47 c3 50 00"],
      [65536, "fa 47 80 00 00"],
      [0.1, "fb 3f b9 99 99 99 99 99 9a"],
    ];
    for (const [x, hex] of cases) {
      const header = encodeHeader([[u(1), { kind: "float", value: x }]]);
      expect([...header.subarray(2)], String(x)).toEqual([...hexToBytes(hex)]);
      expect(decodeHeader(header)).toEqual([[u(1), { kind: "float", value: x }]]);
    }
  });

  it("orders unsigned keys of every head width as their encoded bytes", () => {
    const keys = [65536n, 256n, 24n, 23n, 0n, 255n];
    const header = encodeHeader(keys.map((key) => [u(key), u(0)]));
    expect([...header]).toEqual([...hexToBytes("a6 00 00 17 00 18 18 00 18 ff 00 19 01 00 00 1a 00 01 00 00 00")]);
  });

  it("refuses duplicate keys and reserved or out-of-range simple values on encode", () => {
    expect(encodeError(() => encodeHeader([[u(1), u(0)], [u(1), u(1)]])).kind).toBe("duplicate_key");
    expect(encodeError(() => encodeHeader([[u(1), { kind: "simple", value: 24 }]])).kind).toBe("reserved_simple_value");
    expect(encodeError(() => encodeHeader([[u(1), { kind: "simple", value: 256 }]])).kind).toBe("out_of_range");
  });

  it("treats non-integer duplicate keys by their encoded bytes", () => {
    expect(protocolError(() => decodeHeader(hexToBytes("a2 f5 00 f5 01"))).reason).toBe("duplicate_key");
    expect(decodeHeader(hexToBytes("a2 f9 3c 00 00 fa 3f 80 00 00 01"))).toHaveLength(2);
  });
});

function welcome(limits: Partial<Record<"maxCalls" | "maxFrame" | "maxSubs" | "maxDgram" | "window", bigint>>): Frame {
  return {
    type: "WELCOME",
    version: 1n,
    codec: "cbor",
    caps: [],
    session: "s",
    limits: { maxCalls: 100n, maxFrame: 65_536n, maxSubs: 10n, maxDgram: 1_200n, ...limits },
    hb: 15_000n,
    idle: 45_000n,
  };
}

const samples: Frame[] = [
  {
    type: "HELLO",
    versions: [1n],
    codecs: ["cbor", "json"],
    caps: ["calls"],
    limits: { maxFrame: 65_536n, window: 1n << 20n, maxCalls: 8n },
    contract: new Uint8Array(32).fill(7),
    auth: { scheme: "bearer", token: "secret" },
    resume: { token: Uint8Array.of(1, 2), session: "s1", ack: new Map([[0n, 3n]]), lost: [1n] },
    client: { name: "pavia-ts", version: "0.1.0" },
  },
  welcome({ window: 65_536n }),
  { type: "REJECT", code: 3n, msg: "no", retry: 1000n },
  { type: "PING", data: Uint8Array.of(1, 2, 3, 4, 5, 6, 7, 8) },
  { type: "PONG", data: new Uint8Array(0) },
  { type: "GOAWAY", code: 7n, msg: "drain", url: "wss://x/rt", retry: 5n },
  { type: "CLOSE", code: 0n, msg: "bye" },
  { type: "AUTH_REFRESH", rid: 1n, auth: { scheme: "bearer", token: Uint8Array.of(9) }, seq: 4n },
  { type: "AUTH_RESULT", rid: 1n, code: 0n, expires: 60_000n, ack: new Map([[1n, 2n]]) },
  { type: "ACK", ack: new Map([[0n, 2n ** 64n - 1n]]) },
  { type: "AUTH_EXPIRING", expires: 30_000n },
  { type: "REQUEST_ERROR", rid: 2n, code: 3n, msg: "bad" },
  {
    type: "CALL",
    method: "chat/send",
    shape: 0n,
    deadline: 5000n,
    meta: new Map([["traceparent", "00-1"]]),
    data: Uint8Array.of(0xf6),
  },
  { type: "ITEM", meta: new Map([["k", Uint8Array.of(1)]]), data: Uint8Array.of(0x01) },
  { type: "END" },
  { type: "RESULT", data: Uint8Array.of(0xf6) },
  { type: "ERROR", code: 13n, msg: "boom", nx: true, data: new Uint8Array(0) },
  { type: "CANCEL" },
  { type: "CREDIT", credit: 65_536n },
  { type: "NOTIFY", method: "n", seq: 1n, data: Uint8Array.of(0x80) },
  { type: "LANE_OPEN", lane: 3n },
  {
    type: "SUBSCRIBE",
    rid: 5n,
    channel: "room",
    since: { epoch: "e", offset: 9n },
    presence: true,
    meta: new Map([["a", "b"]]),
    data: new Uint8Array(0),
  },
  { type: "SUBSCRIBED", rid: 5n, channel: "room", epoch: "e", offset: 10n, recovered: false, data: Uint8Array.of(0xa0) },
  { type: "UNSUBSCRIBE", rid: 6n, channel: "room" },
  { type: "UNSUBSCRIBED", channel: "room", form: { rid: 6n } },
  { type: "UNSUBSCRIBED", channel: "room", form: { code: 2n, msg: "resync" } },
  { type: "PUBLICATION", channel: "room", offset: 11n, data: Uint8Array.of(0x01) },
  { type: "PRESENCE", channel: "room", kind: 1n, data: Uint8Array.of(0x80) },
  { type: "DATAGRAM", key: 1n, seq: 2n, data: Uint8Array.of(0x01) },
  { type: "DGRAM_BIND", key: 1n, name: "pos" },
  { type: "EXTENSION", frameType: 0x80, header: Uint8Array.of(0xa0), data: Uint8Array.of(1, 2) },
];

function wireOf(frame: Frame): RawFrame {
  const decoded = takeFrame(encodeFrame(frame), 0, MAX_FRAME_FLOOR);
  if (decoded === undefined) throw new Error("incomplete frame");
  return decoded.frame;
}

describe("typed frames", () => {
  it("round-trips a frame of every registered type", () => {
    for (const frame of samples) {
      const back = decodeFrame(wireOf(frame));
      expect(back, frame.type).toEqual(frame);
      expect(wireOf(frame).flags).toBe(0);
      const header = wireOf(frame).header;
      if (header.length > 0) expect([...header]).toEqual([...encodeHeader(decodeHeader(header) ?? [])]);
    }
  });

  it("writes a frame with no fields with HeaderLength 0", () => {
    expect([...encodeFrame({ type: "END" })]).toEqual([0x12, 0x00, 0x01, 0x00]);
  });

  it("sorts metadata names by length, then bytes", () => {
    const meta = new Map(["ab", "b", "a"].map((name) => [name, ""]));
    expect([...toRawFrame({ type: "END", meta }).header]).toEqual([...hexToBytes("a1 03 a3 61 61 60 61 62 60 62 61 62 60")]);
  });

  it("refuses limits below the 64 KiB floor on encode", () => {
    const hello: Frame = { type: "HELLO", versions: [1n], codecs: ["cbor"], limits: { maxFrame: 32_768n } };
    expect(encodeError(() => encodeFrame(hello)).kind).toBe("limits_below_floor");
    expect(encodeError(() => encodeFrame(welcome({ window: 65_535n }))).kind).toBe("limits_below_floor");
    expect(encodeError(() => encodeFrame(welcome({ maxFrame: 1_024n }))).kind).toBe("limits_below_floor");
    expect(() => encodeFrame(welcome({ maxFrame: 65_536n, window: 65_536n }))).not.toThrow();
  });

  it("refuses values the decoder would refuse on encode", () => {
    const extension: Frame = { type: "EXTENSION", frameType: 0x01, header: new Uint8Array(0), data: new Uint8Array(0) };
    expect(encodeError(() => encodeFrame(extension)).kind).toBe("not_an_extension_type");
    expect(encodeError(() => encodeFrame({ type: "PING", data: new Uint8Array(9) })).kind).toBe("invalid_field");
    expect(encodeError(() => encodeFrame({ type: "END", meta: new Map([["Trace", "x"]]) })).kind).toBe("invalid_field");
    expect(encodeError(() => encodeFrame({ type: "CREDIT", credit: -1n })).kind).toBe("invalid_field");
    expect(encodeError(() => encodeFrame({ type: "CREDIT", credit: 2n ** 64n })).kind).toBe("invalid_field");
    const hello: Frame = { type: "HELLO", versions: [1n], codecs: ["cbor"], contract: new Uint8Array(31) };
    expect(encodeError(() => encodeFrame(hello)).kind).toBe("invalid_field");
  });

  it("refuses unknown must-understand types and keeps extension frames", () => {
    expect(protocolError(() => decodeFrame(raw(0x7f, undefined))).kind).toBe("frame_type_not_allowed");
    const extension = decodeFrame({ type: 0xfe, flags: 3, header: Uint8Array.of(0xff), data: Uint8Array.of(1) });
    expect(extension).toEqual({ type: "EXTENSION", frameType: 0xfe, header: Uint8Array.of(0xff), data: Uint8Array.of(1) });
  });

  it("ignores unknown header keys and flags", () => {
    const frame = decodeFrame({ ...raw(0x07, [[u(6), u(0)], [u(999), t("x")]]), flags: 0xff });
    expect(frame).toEqual({ type: "CLOSE", code: 0n });
  });

  it("refuses a text key at the top of a header", () => {
    const error = protocolError(() => decodeFrame(raw(0x10, [[t("bad"), u(1)], [u(2), t("m")], [u(5), u(0)]])));
    expect([error.kind, error.reason]).toEqual(["malformed_header", "key_not_unsigned"]);
  });

  it("reports a bad entry of a nested map as its outer key", () => {
    const limits: HeaderMap = [
      [u(48), u(100)],
      [u(49), u(65_536)],
      [u(50), u(10)],
      [u(51), u(1_200)],
      [t("window"), u(65_536)],
    ];
    const header: HeaderMap = [
      [u(15), u(1)],
      [u(17), t("cbor")],
      [u(18), { kind: "array", items: [] }],
      [u(19), t("s")],
      [u(21), { kind: "map", entries: limits }],
      [u(22), u(1)],
      [u(23), u(1)],
    ];
    const error = protocolError(() => decodeFrame(raw(0x02, header)));
    expect(error.field).toEqual({ frameType: 0x02, key: 21, problem: "wrong_type", rid: undefined });
    expect(error.outcome).toEqual({ action: "close", code: CloseCode.PROTOCOL_ERROR });
  });

  it("answers a bad control request with REQUEST_ERROR and closes for other frames", () => {
    const subscribe = protocolError(() => decodeFrame(raw(0x30, [[u(1), u(7)]])));
    expect(subscribe.kind).toBe("invalid_control_request_field");
    expect(subscribe.outcome).toEqual({ action: "request_error", rid: 7n, status: 3 });
    const reply = protocolError(() => decodeFrame(raw(0x0c, [[u(1), u(7)]])));
    expect(reply.kind).toBe("invalid_header_field");
    expect(reply.outcome).toEqual({ action: "close", code: CloseCode.PROTOCOL_ERROR });
  });

  it("checks metadata names against [a-z0-9-]", () => {
    const call = (name: string) =>
      raw(0x10, [
        [u(2), t("m")],
        [u(3), { kind: "map", entries: [[t(name), t("v")]] }],
        [u(5), u(0)],
      ]);
    for (const name of ["traceparent", "pavia-idempotency-key", "x-2"]) expect(decodeFrame(call(name)).type).toBe("CALL");
    for (const name of ["", "Trace", "a_b", "a b", "é"]) {
      expect(protocolError(() => decodeFrame(call(name))).field?.key).toBe(3);
    }
  });

  it("refuses UNSUBSCRIBED that mixes or lacks its forms", () => {
    const c: [HeaderValue, HeaderValue] = [u(9), t("c")];
    const cases: [HeaderMap, number, string][] = [
      [[c], 6, "missing"],
      [[c, [u(6), u(2)]], 7, "missing"],
      [[c, [u(1), u(4)], [u(6), u(2)], [u(7), t("m")]], 6, "unexpected"],
      [[c, [u(1), u(4)], [u(7), t("m")]], 7, "unexpected"],
    ];
    for (const [header, key, problem] of cases) {
      const error = protocolError(() => decodeFrame(raw(0x33, header)));
      expect([error.field?.key, error.field?.problem]).toEqual([key, problem]);
    }
  });

  it("refuses data on frame types without a data section", () => {
    expect(protocolError(() => decodeFrame(raw(0x12, undefined, [1]))).kind).toBe("unexpected_data_section");
  });

  it("refuses HELLO limits below the floor and accepts them at the floor", () => {
    for (const key of [49, 40]) {
      const hello = (n: number) =>
        raw(0x01, [
          [u(14), { kind: "array", items: [u(1)] }],
          [u(16), { kind: "array", items: [t("cbor")] }],
          [u(21), { kind: "map", entries: [[u(key), u(n)]] }],
        ]);
      expect(decodeFrame(hello(65_536)).type).toBe("HELLO");
      expect(protocolError(() => decodeFrame(hello(65_535))).field?.problem).toBe("out_of_range");
    }
  });
});

describe("close codes", () => {
  it("maps unknown received codes to INTERNAL", () => {
    expect(closeCodeFromWire(1)).toBe(CloseCode.PROTOCOL_ERROR);
    expect(closeCodeFromWire(13n)).toBe(CloseCode.CONTRACT_MISMATCH);
    expect(closeCodeFromWire(14)).toBe(CloseCode.INTERNAL);
    expect(closeCodeFromWire(2n ** 64n)).toBe(CloseCode.INTERNAL);
  });
});

describe("text with a lone surrogate", () => {
  it("is refused rather than rewritten as U+FFFD", () => {
    const header: HeaderMap = [[{ kind: "uint", value: 1n }, { kind: "text", value: "room-\uD83D" }]];
    expect(encodeError(() => encodeHeader(header)).kind).toBe("lone_surrogate");
    const subscribe = { type: "SUBSCRIBE", rid: 1n, channel: "room-\uD83D", data: new Uint8Array() } as Frame;
    expect(encodeError(() => encodeFrame(subscribe)).kind).toBe("lone_surrogate");
    const close = { type: "CLOSE", code: 0n, msg: "\uDC00" } as Frame;
    expect(encodeError(() => encodeFrame(close)).kind).toBe("lone_surrogate");
  });

  it("does not catch a well-formed surrogate pair", () => {
    const header: HeaderMap = [[{ kind: "uint", value: 1n }, { kind: "text", value: "😀" }]];
    expect(Array.from(encodeHeader(header), (byte) => byte.toString(16).padStart(2, "0")).join("")).toBe(
      "a10164f09f9880",
    );
  });
});
