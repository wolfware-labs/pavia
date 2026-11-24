import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { StatusCode, StatusError } from "../src/index.js";
import type { Json } from "./support.js";
import { type CallOutcome, clientCases, runClientScript } from "./conformance.js";

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

const REFUSED_HEADER = `{15: 2, 17: "cbor", 18: ["calls"], 19: "s_test", 21: ${LIMITS}, 22: 15000, 23: 45000}`;
const REFUSED = { expect: { stream: 0, type: 2, header: REFUSED_HEADER, data: null } };

function script(steps: Json[]): Json {
  return { id: "self-test", spec: ["17.2"], note: "", binding: "ws", codec: "cbor", steps };
}

function byId(id: string): Json {
  const found = clientCases().find((x) => x.id.endsWith(`#${id}`));
  if (found === undefined) throw new Error(`no client script ${id}`);
  return found.c;
}

describe("calls in client scripts", () => {
  async function outcomes(id: string): Promise<CallOutcome[]> {
    const run = await runClientScript(byId(id));
    return Promise.all(run.calls.values());
  }

  function failed(outcome: CallOutcome | undefined): StatusError {
    expect(outcome?.ok).toBe(false);
    const error = (outcome as { error: unknown }).error;
    expect(error).toBeInstanceOf(StatusError);
    return error as StatusError;
  }

  it("resolve with the RESULT's value", async () => {
    const [call] = await outcomes("client-call-unary-result");
    expect(call).toEqual({ ok: true, value: { kind: "uint", value: 42n }, trailers: undefined });
  });

  it("reject an aborted call with CANCELLED, without nx, and free its stream", async () => {
    const [aborted, next] = await outcomes("client-call-abort");
    expect([failed(aborted).code, failed(aborted).nx]).toEqual([StatusCode.CANCELLED, false]);
    expect(next?.ok).toBe(true);
  });

  it("reject a call whose deadline passed with DEADLINE_EXCEEDED", async () => {
    const [call] = await outcomes("client-call-deadline");
    expect([failed(call).code, failed(call).nx]).toEqual([StatusCode.DEADLINE_EXCEEDED, false]);
  });

  it("expose an ERROR's code, message, detail and nx", async () => {
    const error = failed((await outcomes("client-call-error-detail"))[0]);
    const detail = { kind: "map", entries: [[{ kind: "text", value: "reason" }, { kind: "text", value: "banned" }]] };
    const fields = [error.code, error.message, error.detail, error.nx];
    expect(fields).toEqual([StatusCode.PERMISSION_DENIED, "denied", detail, true]);
  });

  it("complete an undecodable RESULT with INTERNAL and go on", async () => {
    const [bad, next] = await outcomes("client-call-undecodable-result");
    expect(failed(bad).code).toBe(StatusCode.INTERNAL);
    expect(next).toEqual({ ok: true, value: { kind: "uint", value: 1n }, trailers: undefined });
  });

  it("map an unknown status code to UNKNOWN", async () => {
    expect(failed((await outcomes("client-call-unknown-status"))[0]).code).toBe(StatusCode.UNKNOWN);
  });
});

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

  it("fails when connect() rejects and no step expects the close", async () => {
    for (const steps of [[HELLO, REFUSED], [HELLO, REFUSED, { wait: { ms: 100 } }]]) {
      await expect(runClientScript(script(steps))).rejects.toThrow("connect() failed and no step expects it");
    }
    const closes = { expect_close: { code: "PROTOCOL_ERROR", within_ms: 1000 } };
    expect((await runClientScript(script([HELLO, REFUSED, closes]))).connection).toBeUndefined();
    expect((await runClientScript(script([HELLO, { close_transport: {} }]))).connection).toBeUndefined();
  });

  it("fails a step that acts on a transport the client already closed", async () => {
    const ping = { expect: { stream: 0, type: 4, header: null, data: null } };
    const drop = { close_transport: {} };
    const cases: [Json[], string][] = [
      [[HELLO, REFUSED, drop], "step 3 (close_transport)"],
      [[HELLO, REFUSED, ping], "step 3 (expect)"],
      [[HELLO, drop, drop], "step 3 (close_transport)"],
      [[HELLO, welcome(), drop, ping], "step 4 (expect)"],
    ];
    for (const [steps, where] of cases) {
      await expect(runClientScript(script(steps))).rejects.toThrow(`${where}: the transport is already closed`);
    }
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

  it("fails expect_nothing_within on a frame sent before it and on a closed transport", async () => {
    const quiet = { expect_nothing_within: { ms: 100 } };
    const unread = "step 3 (expect_nothing_within): the client sent 1 frames";
    await expect(runClientScript(script([HELLO, REFUSED, quiet]))).rejects.toThrow(unread);
    const bye = { send: { stream: 0, type: 7, header: '{6: 0, 7: "bye"}', data: null } };
    const closes = [HELLO, welcome(), bye, { expect_nothing_within: { ms: 2000 } }];
    const closed = "step 4 (expect_nothing_within): the client closed its transport with 4000";
    await expect(runClientScript(script(closes))).rejects.toThrow(closed);
    const dropped = [HELLO, welcome(), { close_transport: {} }, quiet];
    const ended = "step 4 (expect_nothing_within): the transport is already closed";
    await expect(runClientScript(script(dropped))).rejects.toThrow(ended);
  });

  it("leaves the client's PONG out of expect_nothing_within", async () => {
    const ping = { expect: { stream: 0, type: 4, header: null, data: null, data_hex: "01 02" } };
    await runClientScript(script([HELLO, welcome(), ping, { expect_nothing_within: { ms: 100 } }]));
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
    const sent = { send: { ...(HELLO.send as Json), capture: { version: "header.14" } } };
    await expect(runClientScript(script([sent]))).rejects.toThrow("captures nothing");
  });

  it("sends and matches a json data section as its exact text", async () => {
    const data = '{"n":1}';
    const variants: [string, Json][] = [
      ["json", {}],
      ["cbor", { codec: "json" }],
      ["json", { data_hex: "01" }],
    ];
    for (const [codec, extra] of variants) {
      const frame = { stream: 1, type: 17, header: null, data, ...extra };
      const sent: number[][] = [];
      await runClientScript({ ...script([{ ...HELLO }, { ...welcome() }, { expect: frame }]), codec }, {
        onConnection: (connection) =>
          connection.addEventListener("frame", (event) => {
            const { frame } = (event as CustomEvent<{ frame: { type: string; data?: Uint8Array } }>).detail;
            sent.push([...(frame.data ?? [])]);
          }),
      });
      expect(sent).toEqual([[...new TextEncoder().encode(data)]]);
    }
  });

  it("refuses an unknown codec", async () => {
    await expect(runClientScript({ ...script([HELLO]), codec: "xml" })).rejects.toThrow("unknown codec xml");
  });

  it("refuses a script whose first step is not a send of HELLO", async () => {
    const ping = { send: { stream: 0, type: 4, header: null, data: null } };
    for (const steps of [[], [ping], [{ wait: { ms: 1 } }, HELLO], [welcome(), HELLO]]) {
      await expect(runClientScript(script(steps))).rejects.toThrow("the first step is not a send of HELLO");
    }
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
