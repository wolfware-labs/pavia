/**
 * Application code of the example chat app, written against the client generated from the example
 * manifest in `./contract.ts`. The exit-criterion job regenerates that module from a changed Rust
 * signature and expects the typecheck of this file to fail at the call of `chat/send`.
 */
import { type ChatClient, type ChatHandlers, ChatSendError, type MessageId, type RoomId } from "./contract.js";

/** What became of a message: its ID once the server took it, or why the server refused it. */
export type Posted = { sent: true; id: MessageId } | { sent: false; reason: string };

/** What the app needs from its user interface to answer the server's calls. */
export interface Screen {
  ask(prompt: string): boolean | Promise<boolean>;
  show(line: string): void;
}

/** Sends `text` to `room`, and turns the server's typed refusal into a reason to show the user. */
export async function postMessage(chat: Pick<ChatClient, "send">, room: RoomId, text: string): Promise<Posted> {
  try {
    const id = await chat.send(room, text);
    return { sent: true, id };
  } catch (error) {
    if (error instanceof ChatSendError) return { sent: false, reason: reason(error, room) };
    throw error;
  }
}

/** Tells the room that the user is typing. */
export function startTyping(chat: Pick<ChatClient, "typing">, room: RoomId): void {
  chat.typing(room);
}

/** The handlers of the calls the server makes on this client, answered through `screen`. */
export function chatHandlers(screen: Screen): ChatHandlers {
  return {
    confirm: (prompt) => screen.ask(prompt),
    systemNotice: (text) => screen.show(`notice: ${text}`),
  };
}

function reason(error: ChatSendError, room: RoomId): string {
  const detail = error.detail;
  switch (detail?.kind) {
    case "RoomNotFound":
      return `room ${room} does not exist`;
    case "TooLong":
      return `messages are limited to ${detail.value.max} characters`;
    default:
      return error.message;
  }
}
