import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { type CborItem, type Codec, decodeData, encodeData, type Types } from "../src/index.js";
import type { Message as CamelMessage, SendError as CamelSendError } from "./generated/contract-camel.js";
import type { Message, SendError } from "./generated/contract.js";

const ID = "0b6a4c3e-1d2f-4a5b-8c7d-9e0f1a2b3c4d";

/** The types table a generated module embeds, read from its text, since the module does not export it. */
function typesOf(module: string): Types {
  const text = readFileSync(new URL(`./generated/${module}`, import.meta.url), "utf8");
  const start = text.indexOf("const TYPES: Types = {\n");
  const end = text.indexOf("\n};\n", start);
  expect(start, module).toBeGreaterThan(-1);
  const entries = text.slice(start + "const TYPES: Types = {\n".length, end).replace(/,$/, "");
  return JSON.parse(`{${entries}}`) as Types;
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

describe("the example app's module under each naming policy", () => {
  for (const [module, value, error] of [
    ["contract.ts", message, tooLong],
    ["contract-camel.ts", camelMessage, camelTooLong],
  ] as const) {
    const types = typesOf(module);

    it(`${module}: sends a Message with the manifest's field names in declaration order and reads it back`, () => {
      for (const codec of ["cbor", "json"] as const) {
        const bytes = encodeData({ ref: "Message" }, types, value, codec);
        expect(wireKeys(bytes, codec), codec).toEqual(["id", "author", "text", "sent_at"]);
        expect(decodeData({ ref: "Message" }, types, bytes, codec), codec).toEqual(value);
      }
    });

    it(`${module}: keeps the variant TooLong as it is on the wire`, () => {
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
