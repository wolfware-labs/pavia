import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Json } from "./support.js";
import { clientCases, runClientScript } from "./conformance.js";

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.clearAllTimers();
  vi.useRealTimers();
});

describe("client scripts", () => {
  it("are found", () => {
    expect(clientCases().length).toBeGreaterThan(0);
  });

  for (const { id, c } of clientCases()) {
    it(id, async () => {
      await runClientScript(c);
    });
  }
});

const HELLO = { send: { stream: 0, type: 1, header: '{14: [1], 16: ["cbor"], 18: ["calls"], ...}', data: null } };
const LIMITS = "{40: 262144, 48: 100, 49: 1048576, 50: 256, 51: 1100}";

function welcome(hb = 15000, idle = 45000): Json {
  const header = `{15: 1, 17: "cbor", 18: ["calls"], 19: "s_test", 21: ${LIMITS}, 22: ${hb}, 23: ${idle}}`;
  return { expect: { stream: 0, type: 2, header, data: null } };
}

function script(steps: Json[]): Json {
  return { id: "self-test", spec: ["17.2"], note: "", binding: "ws", codec: "cbor", steps };
}

function byId(id: string): Json {
  const found = clientCases().find((x) => x.id.endsWith(`#${id}`));
  if (found === undefined) throw new Error(`no client script ${id}`);
  return found.c;
}

describe("the client runner", () => {
  it("reports one contractMismatch event and one warning for the mismatch script", async () => {
    let events = 0;
    const run = await runClientScript(byId("client-contract-mismatch-stays-open"), {
      onConnection: (connection) => connection.addEventListener("contractMismatch", () => events++),
    });
    expect([events, run.warnings.length]).toEqual([1, 1]);
    expect(run.connection?.closedWith).toBeUndefined();
  });

  it("configures the client from the first HELLO send", async () => {
    const json = { send: { stream: 0, type: 1, header: '{16: ["json"], ...}', data: null } };
    await runClientScript(script([json]));
  });

  it("reports no mismatch when the fingerprints agree", async () => {
    const run = await runClientScript(byId("client-handshake-answers-ping"));
    expect([run.connection?.contractMismatch, run.warnings]).toEqual([undefined, []]);
  });

  it("names the step whose send does not match", async () => {
    const hello = { send: { stream: 0, type: 1, header: '{14: [1], 16: ["json"], ...}', data: null } };
    await expect(runClientScript(script([HELLO, hello]))).rejects.toThrow("step 2 (send): the client sent nothing");
    const version = { send: { stream: 0, type: 1, header: "{14: [2], ...}", data: null } };
    await expect(runClientScript(script([version]))).rejects.toThrow("step 1 (send): header does not match");
  });

  it("fails on a closed map with keys the client sends besides", async () => {
    const closed = { send: { stream: 0, type: 1, header: '{14: [1], 16: ["cbor"], 18: ["calls"]}', data: null } };
    await expect(runClientScript(script([closed]))).rejects.toThrow("step 1 (send): header does not match");
  });

  it("fails on another close code", async () => {
    const steps = [HELLO, welcome(), { expect_close: { code: "NORMAL", within_ms: 50 } }];
    await expect(runClientScript(script(steps))).rejects.toThrow("step 3 (expect_close): the client did not close");
    const late = [HELLO, welcome(60000, 300), { expect_close: { code: "NORMAL", within_ms: 1000 } }];
    await expect(runClientScript(script(late))).rejects.toThrow("closed with 4005, not 4000");
  });

  it("fails when the client sends during expect_nothing_within", async () => {
    const steps = [HELLO, welcome(), { send: { stream: 0, type: 7, header: "{6: 0}", data: null } }];
    await runClientScript(script(steps));
    const quiet = [HELLO, { expect_nothing_within: { ms: 11000 } }];
    await expect(runClientScript(script(quiet))).rejects.toThrow("step 2 (expect_nothing_within): the client sent 1");
  });

  it("answers the client's PING on its own unless a step sends PING", async () => {
    const steps = [HELLO, welcome(100, 400), { wait: { ms: 1000 } }, { expect_nothing_within: { ms: 50 } }];
    const run = await runClientScript(script(steps));
    expect(run.connection?.closedWith).toBeUndefined();
  });

  it("refuses an expect with a pattern and a capture", async () => {
    const pattern = { expect: { stream: 0, type: 2, header: "{15: 1, ...}", data: null } };
    await expect(runClientScript(script([HELLO, pattern]))).rejects.toThrow("an expect is concrete");
    const capture = { expect: { ...(welcome().expect as Json), capture: { session: "header.19" } } };
    await expect(runClientScript(script([HELLO, capture]))).rejects.toThrow("captures nothing");
  });

  it("refuses a script whose first send is not HELLO", async () => {
    const steps = [{ send: { stream: 0, type: 4, header: null, data: null } }];
    await expect(runClientScript(script(steps))).rejects.toThrow("the first send is not HELLO");
  });

  it("refuses a step that is not one verb, an unknown close code name and a $name in a send", async () => {
    for (const step of [{}, { wait: { ms: 1 }, expect_nothing_within: { ms: 1 } }]) {
      await expect(runClientScript(script([HELLO, step]))).rejects.toThrow("step 2: a step has exactly one verb");
    }
    const unknown = { expect_close: { code: "CLOSED", within_ms: 50 } };
    await expect(runClientScript(script([HELLO, unknown]))).rejects.toThrow("step 2: unknown close code CLOSED");
    const send = { send: { stream: 0, type: 1, header: "{14: $versions, ...}", data: null } };
    await expect(runClientScript(script([send]))).rejects.toThrow("step 1: a client script uses no $name");
  });
});
