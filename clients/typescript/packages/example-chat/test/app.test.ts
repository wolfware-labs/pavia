import { StatusCode, StatusError } from "@pavia/client";
import { describe, expect, it } from "vitest";
import { chatHandlers, postMessage, startTyping } from "../src/app.js";
import { type ChatClient, ChatSendError, type SendError } from "../src/contract.js";

const ID = "0b6a4c3e-1d2f-4a5b-8c7d-9e0f1a2b3c4d";

function sending(calls: unknown[][]): Pick<ChatClient, "send"> {
  return {
    send: (...args) => {
      calls.push(args);
      return Promise.resolve(ID);
    },
  };
}

function refusing(error: Error): Pick<ChatClient, "send"> {
  return { send: () => Promise.reject(error) };
}

function refusal(detail: SendError | undefined, message = "refused"): ChatSendError {
  return new ChatSendError(StatusCode.INVALID_ARGUMENT, message, { detail });
}

describe("postMessage", () => {
  it("sends the room and the text and returns the message ID", async () => {
    const calls: unknown[][] = [];
    await expect(postMessage(sending(calls), "lobby", "hi")).resolves.toEqual({ sent: true, id: ID });
    expect(calls).toEqual([["lobby", "hi"]]);
  });

  it("says the room does not exist when the server answers RoomNotFound", async () => {
    await expect(postMessage(refusing(refusal({ kind: "RoomNotFound" })), "attic", "hi")).resolves.toEqual({
      sent: false,
      reason: "room attic does not exist",
    });
  });

  it("gives the length limit when the server answers TooLong", async () => {
    const posted = postMessage(refusing(refusal({ kind: "TooLong", value: { max: 4000 } })), "lobby", "x");
    await expect(posted).resolves.toEqual({ sent: false, reason: "messages are limited to 4000 characters" });
  });

  it("falls back to the error message when the refusal has no detail", async () => {
    await expect(postMessage(refusing(refusal(undefined, "unavailable")), "lobby", "hi")).resolves.toEqual({
      sent: false,
      reason: "unavailable",
    });
  });

  it("lets any other error through", async () => {
    const other = new StatusError(StatusCode.INTERNAL, "boom");
    await expect(postMessage(refusing(other), "lobby", "hi")).rejects.toBe(other);
  });
});

describe("startTyping", () => {
  it("sends a typing notification for the room", () => {
    const rooms: string[] = [];
    startTyping({ typing: (room) => void rooms.push(room) }, "lobby");
    expect(rooms).toEqual(["lobby"]);
  });
});

describe("chatHandlers", () => {
  it("asks the user to confirm and shows system notices", async () => {
    const shown: string[] = [];
    const handlers = chatHandlers({
      ask: (prompt) => prompt === "leave the room?",
      show: (line) => void shown.push(line),
    });
    expect(await handlers.confirm("leave the room?")).toBe(true);
    expect(await handlers.confirm("delete everything?")).toBe(false);
    await handlers.systemNotice("maintenance at noon");
    expect(shown).toEqual(["notice: maintenance at noon"]);
  });
});
