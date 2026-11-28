import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  type CborItem,
  type Codec,
  type Connection,
  decodeData,
  encodeData,
  type NotifySignature,
  type Types,
} from "../src/index.js";
import { establishedQuietly, FakeSocket } from "./fake-socket.js";
import {
  ChatClient as CamelChatClient,
  type Message as CamelMessage,
  type SendError as CamelSendError,
} from "./generated/contract-camel.js";
import { ChatClient, type Message, type SendError } from "./generated/contract.js";

const ID = "0b6a4c3e-1d2f-4a5b-8c7d-9e0f1a2b3c4d";

/** The types table a generated module hands the runtime, taken from the signature `chat/typing` sends with. */
async function typesOf(client: (connection: Connection) => { typing(room: string): void }): Promise<Types> {
  const { connection } = await establishedQuietly();
  const notify = vi.spyOn(connection, "notify");
  client(connection).typing("lobby");
  const signature = notify.mock.calls[0]?.[0] as NotifySignature;
  expect(signature.method).toBe("chat/typing");
  return signature.types;
}

function wireKeys(bytes: Uint8Array, codec: Codec): string[] {
  if (codec === "json") return Object.keys(JSON.parse(new TextDecoder().decode(bytes)) as object);
  const item = decodeData("any", {}, bytes, "cbor") as Extract<CborItem, { kind: "map" }>;
  return item.entries.map(([key]) => (key as { value: string }).value);
}

const message: Message = { id: ID, author: "ada", text: "hi", sent_at: new Date(1000) };
const camelMessage: CamelMessage = { id: ID, author: "ada", text: "hi", sentAt: new Date(1000) };
const tooLong: SendError = { kind: "TooLong", value: { max: 4000 } };
const camelTooLong: CamelSendError = { kind: "TooLong", value: { max: 4000 } };

beforeEach(() => {
  vi.useFakeTimers();
  FakeSocket.last = undefined;
  FakeSocket.count = 0;
});

afterEach(() => {
  vi.useRealTimers();
});

describe("the example app's module under each naming policy", () => {
  for (const [module, client, value, error] of [
    ["contract.ts", (connection: Connection) => new ChatClient(connection), message, tooLong],
    ["contract-camel.ts", (connection: Connection) => new CamelChatClient(connection), camelMessage, camelTooLong],
  ] as const) {
    it(`${module}: sends a Message with the manifest's field names in order and reads it back`, async () => {
      const types = await typesOf(client);
      for (const codec of ["cbor", "json"] as const) {
        const bytes = encodeData({ ref: "Message" }, types, value, codec);
        expect(wireKeys(bytes, codec), codec).toEqual(["id", "author", "text", "sent_at"]);
        expect(decodeData({ ref: "Message" }, types, bytes, codec), codec).toEqual(value);
      }
    });

    it(`${module}: keeps the variant TooLong as it is on the wire`, async () => {
      const types = await typesOf(client);
      const bytes = encodeData({ ref: "SendError" }, types, error, "json");
      expect(new TextDecoder().decode(bytes)).toBe('{"TooLong":{"max":4000}}');
      for (const codec of ["cbor", "json"] as const) {
        const encoded = encodeData({ ref: "SendError" }, types, error, codec);
        expect(wireKeys(encoded, codec), codec).toEqual(["TooLong"]);
        expect(decodeData({ ref: "SendError" }, types, encoded, codec), codec).toEqual(error);
      }
    });
  }
});
