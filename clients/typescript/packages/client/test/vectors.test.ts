import { describe, expect, it } from "vitest";
import {
  CloseCode,
  decodeFrame,
  decodeHeader,
  decodeMessage,
  decodeVarint,
  encodeFrame,
  encodeHeader,
  encodeHeaderValue,
  encodeMessages,
  encodeVarint,
  EncodeError,
  type Frame,
  MAX_FRAME_FLOOR,
  ProtocolError,
  type RawFrame,
  StreamFrameDecoder,
  toRawFrame,
} from "../src/index.js";
import { cases, concat, headerOf, hexToBytes, type Json, valueOf } from "./support.js";

function thrown(f: () => unknown): unknown {
  try {
    f();
  } catch (error) {
    return error;
  }
  throw new Error("expected an error");
}

describe("varint vectors", () => {
  const all = cases("varints");
  it("has cases", () => expect(all.length).toBeGreaterThan(0));
  for (const { id, c } of all) {
    it(id, () => {
      const max = typeof c.max === "string" ? BigInt(c.max) : undefined;
      if (c.error === "out_of_range") {
        const error = thrown(() => encodeVarint(BigInt(c.value as string)));
        expect(error).toBeInstanceOf(EncodeError);
        expect((error as EncodeError).kind).toBe("out_of_range");
        return;
      }
      const bytes = hexToBytes(c.hex as string);
      const decoded = decodeVarint(bytes, 0, max);
      if (c.need_more === true) {
        expect(decoded).toEqual({ kind: "need_more" });
      } else if (c.error === "above_max") {
        expect(decoded.kind).toBe("above_max");
      } else {
        expect(decoded).toEqual({ kind: "value", value: BigInt(c.value as string), length: bytes.length });
        if (c.direction === "both") expect([...encodeVarint(BigInt(c.value as string))]).toEqual([...bytes]);
        else expect([...encodeVarint(BigInt(c.value as string))]).not.toEqual([...bytes]);
      }
    });
  }
});

describe("header vectors", () => {
  const all = cases("headers");
  it("has cases", () => expect(all.length).toBeGreaterThan(0));
  for (const { id, c } of all) {
    it(id, () => {
      const bytes = hexToBytes(c.hex as string);
      const limits = {
        maxSize: typeof c.max_size === "number" ? c.max_size : 16_384,
        maxDepth: typeof c.max_depth === "number" ? c.max_depth : 16,
      };
      if (typeof c.error === "string") {
        const error = thrown(() => decodeHeader(bytes, limits));
        expect(error).toBeInstanceOf(ProtocolError);
        expect((error as ProtocolError).kind).toBe("malformed_header");
        expect((error as ProtocolError).reason).toBe(c.error);
        expect((error as ProtocolError).outcome).toEqual({ action: "close", code: CloseCode.PROTOCOL_ERROR });
        return;
      }
      const expected = headerOf(c.header as string);
      const decoded = decodeHeader(bytes, limits);
      expect(decoded).toEqual(expected);
      const encoded = encodeHeader(expected);
      if (c.direction === "both") expect([...encoded]).toEqual([...bytes]);
      else expect([...encoded]).not.toEqual([...bytes]);
      expect([...encodeHeader(decoded ?? [])]).toEqual([...encoded]);
    });
  }
});

function expectedData(f: Json): Uint8Array {
  if (typeof f.data_hex === "string") return hexToBytes(f.data_hex);
  if (typeof f.data !== "string") return new Uint8Array(0);
  return f.codec === "json" ? new TextEncoder().encode(f.data) : encodeHeaderValue(valueOf(f.data));
}

function rawFrames(c: Json, bytes: Uint8Array): { stream?: number; frame: RawFrame }[] {
  if (c.binding === "ws") return decodeMessage(bytes, MAX_FRAME_FLOOR);
  const decoder = new StreamFrameDecoder(MAX_FRAME_FLOOR);
  decoder.push(bytes);
  const out: { frame: RawFrame }[] = [];
  for (let frame = decoder.next(); frame !== undefined; frame = decoder.next()) out.push({ frame });
  if (c.fin === true) decoder.finish();
  return out;
}

function encoded(c: Json, frames: readonly { stream?: number; frame: Frame }[]): Uint8Array {
  if (c.binding === "ws") {
    const messages = encodeMessages(frames.map(({ stream, frame }) => ({ stream: stream ?? 0, frame: toRawFrame(frame) })));
    expect(messages.length).toBe(1);
    return messages[0] as Uint8Array;
  }
  return concat(frames.map(({ frame }) => encodeFrame(frame)));
}

const CLOSE_CODES: Record<string, number> = CloseCode;

describe("frame vectors", () => {
  const all = cases("frames");
  it("has cases", () => expect(all.length).toBeGreaterThan(0));
  for (const { id, c } of all) {
    it(id, () => {
      const bytes = hexToBytes(c.hex as string);
      const error = c.error as { code: string } | undefined;
      if (error !== undefined) {
        const e = thrown(() => rawFrames(c, bytes).forEach(({ frame }) => decodeFrame(frame)));
        expect(e).toBeInstanceOf(ProtocolError);
        expect((e as ProtocolError).outcome).toEqual({ action: "close", code: CLOSE_CODES[error.code] });
        return;
      }
      const want = c.frames as Json[];
      const raw = rawFrames(c, bytes);
      expect(raw.length).toBe(want.length);
      const typed = raw.map(({ stream, frame }, i) => {
        const w = want[i] as Json;
        if (c.binding === "ws") expect(stream).toBe(w.stream);
        expect(frame.type).toBe(w.type);
        expect(frame.flags).toBe(w.flags);
        if (typeof w.header === "string") {
          const decoded = encodeHeader(decodeHeader(frame.header) ?? []);
          expect([...decoded]).toEqual([...encodeHeader(headerOf(w.header))]);
        } else {
          expect(frame.header.length).toBe(0);
        }
        expect([...frame.data]).toEqual([...expectedData(w)]);
        const t = decodeFrame(frame);
        expect(toRawFrame(t).data).toEqual(frame.data);
        return stream === undefined ? { frame: t } : { stream, frame: t };
      });
      if (typed.length === 0) return;
      const out = encoded(c, typed);
      if (c.direction === "both") {
        expect([...out]).toEqual([...bytes]);
      } else {
        for (const { frame } of typed) {
          const raw = toRawFrame(frame);
          expect(raw.flags).toBe(0);
          if (raw.header.length > 0) expect([...raw.header]).toEqual([...encodeHeader(decodeHeader(raw.header) ?? [])]);
        }
      }
    });
  }
});
