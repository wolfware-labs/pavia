import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { encodeData, StatusCode, StatusError, type Types } from "../src/index.js";
import { establishedQuietly, FakeSocket } from "./fake-socket.js";
import { failure } from "./support.js";
import { ChatClient, ChatSendError, CONTRACT, type SendError } from "./generated/contract.js";

const SEND_ERROR: Types = {
  SendError: {
    enum: {
      variants: [
        { name: "RoomNotFound" },
        { name: "TooLong", type: { struct: { fields: [{ name: "max", type: "u32" }] } } },
      ],
    },
  },
};

async function open() {
  const { connection, ws } = await establishedQuietly();
  return { connection, ws, chat: new ChatClient(connection) };
}

beforeEach(() => {
  vi.useFakeTimers();
  FakeSocket.last = undefined;
  FakeSocket.count = 0;
});

afterEach(() => {
  vi.useRealTimers();
});

describe("the generated client of the example app", () => {
  it("calls chat/send with its arguments and resolves with the message ID", async () => {
    const { ws, chat } = await open();
    const pending = chat.send("lobby", "hi");
    const sent = ws.sentOn.filter(({ frame }) => frame.type === "CALL").map(({ frame }) => frame);
    expect(sent).toEqual([{
      type: "CALL",
      method: "chat/send",
      shape: 0n,
      data: encodeData({ tuple: ["string", "string"] }, {}, ["lobby", "hi"], "cbor"),
    }]);
    const id = "0b6a4c3e-1d2f-4a5b-8c7d-9e0f1a2b3c4d";
    ws.receive({ type: "RESULT", data: encodeData("uuid", {}, id, "cbor") }, 2);
    await expect(pending).resolves.toBe(id);
  });

  it("rejects chat/send with a ChatSendError whose detail is the decoded SendError", async () => {
    const { ws, chat } = await open();
    const pending = chat.send("lobby", "x".repeat(5000));
    const detail: SendError = { kind: "TooLong", value: { max: 4000 } };
    const data = encodeData({ ref: "SendError" }, SEND_ERROR, detail, "cbor");
    ws.receive({ type: "ERROR", code: 3n, msg: "too long", data }, 2);
    const error = await failure(pending);
    expect(error).toBeInstanceOf(ChatSendError);
    expect(error).toBeInstanceOf(StatusError);
    const failed = error as ChatSendError;
    expect([failed.name, failed.code, failed.message, failed.detail, failed.nx]).toEqual([
      "ChatSendError",
      StatusCode.INVALID_ARGUMENT,
      "too long",
      detail,
      false,
    ]);
  });

  it("passes a local refusal through as a ChatSendError with nx", async () => {
    const { ws, chat } = await open();
    ws.receive({ type: "CLOSE", code: 7n });
    const error = await failure(chat.send("lobby", "hi"));
    expect(error).toBeInstanceOf(ChatSendError);
    expect([(error as ChatSendError).code, (error as ChatSendError).nx]).toEqual([StatusCode.UNAVAILABLE, true]);
  });

  it("sends chat/typing as a notification", async () => {
    const { ws, chat } = await open();
    expect(chat.typing("lobby")).toBeUndefined();
    expect(ws.sentOn.filter(({ frame }) => frame.type === "NOTIFY")).toEqual([
      {
        stream: 0,
        frame: {
          type: "NOTIFY",
          method: "chat/typing",
          data: encodeData({ tuple: ["string"] }, {}, ["lobby"], "cbor"),
        },
      },
    ]);
  });

  it("throws, for a streaming method, that streaming is not implemented until M5", async () => {
    const { chat } = await open();
    expect(() => chat.history("lobby")).toThrow("chat/history: streaming is not implemented until M5");
    async function* none(): AsyncIterable<Uint8Array> {}
    await expect(chat.upload(none())).rejects.toThrow("chat/upload: streaming is not implemented until M5");
  });

  it("holds the 32-byte contract fingerprint", () => {
    expect(CONTRACT).toBeInstanceOf(Uint8Array);
    expect(CONTRACT.length).toBe(32);
  });
});
