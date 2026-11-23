import { describe, expect, it } from "vitest";
import { CloseCode, type Connection, connect, type Ending } from "../src/index.js";

const URL = process.env["PAVIA_TS_PEER_URL"];
const HOLD_INTERVALS = 5;

function after(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

describe.skipIf(URL === undefined)("against the Rust server (crates/pavia-server/examples/ts-peer.rs)", () => {
  it("completes the handshake and holds the session through several hb intervals", async () => {
    const connection: Connection = await connect(URL as string, { codecs: ["cbor"] });
    const hb = Number(connection.welcome.hb);
    const idle = Number(connection.welcome.idle);
    expect([connection.welcome.version, connection.welcome.codec, hb < idle]).toEqual([1n, "cbor", true]);
    let ended: Ending | undefined;
    connection.addEventListener("close", (event) => {
      ended = (event as CustomEvent<Ending>).detail;
    });
    const hold = Math.max(HOLD_INTERVALS * hb, idle * 2);
    await after(hold);
    expect(ended).toBeUndefined();
    const ending = await connection.close(CloseCode.NORMAL, "done");
    expect(ending).toMatchObject({ kind: "closed", by: "local", code: CloseCode.NORMAL });
  }, 20_000);
});
