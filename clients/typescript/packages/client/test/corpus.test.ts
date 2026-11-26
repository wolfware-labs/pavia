import { describe, expect, expectTypeOf, it } from "vitest";
import type { Holder, HolderInput, Status, StatusInput, _2faClient, _2faHandlers } from "./generated/corpus.js";

type CatchAll = { kind: "Unknown"; name: string };

describe("the generated corpus", () => {
  it("takes what a client sends in the input form, which has no catch-all", () => {
    expectTypeOf<Parameters<_2faClient["verify"]>[1]>().toEqualTypeOf<StatusInput>();
    expectTypeOf<Parameters<_2faClient["tell"]>[0]>().toEqualTypeOf<StatusInput>();
    expectTypeOf<Parameters<_2faClient["upload"]>[0]>().toEqualTypeOf<AsyncIterable<StatusInput>>();
    expectTypeOf<Parameters<_2faClient["watch"]>[0]>().toEqualTypeOf<HolderInput>();
    expectTypeOf<CatchAll>().not.toExtend<StatusInput>();
    expectTypeOf<Status>().not.toExtend<StatusInput>();
    expectTypeOf<Holder>().not.toExtend<HolderInput>();
  });

  it("gives what a client receives in the decoded form, catch-all included", () => {
    expectTypeOf<ReturnType<_2faClient["verify"]>>().toEqualTypeOf<Promise<Holder>>();
    expectTypeOf<ReturnType<_2faClient["watch"]>>().toEqualTypeOf<AsyncIterable<Status>>();
    expectTypeOf<CatchAll>().toExtend<Status>();
    expectTypeOf<Parameters<_2faHandlers["new"]>[0]>().toEqualTypeOf<Status>();
    expectTypeOf<Parameters<_2faHandlers["get"]>[0]>().toEqualTypeOf<Holder>();
    expectTypeOf<Parameters<_2faHandlers["feed"]>[0]>().toEqualTypeOf<AsyncIterable<Status>>();
  });

  it("lets an application implement every handler, one named new included", async () => {
    const seen: string[] = [];
    const handlers: _2faHandlers = {
      new(status) {
        return status.kind === "Unknown" ? { kind: "Idle" } : status;
      },
      get(holder) {
        seen.push(holder.class);
      },
      delete() {
        seen.push("delete");
      },
      static() {},
      async *feed(items) {
        for await (const item of items) {
          if (item.kind !== "Unknown") yield item;
        }
      },
    };
    expect(handlers.new({ kind: "Unknown", name: "Away" })).toEqual({ kind: "Idle" });
    await handlers.get({ status: { kind: "Idle" }, new: 1, class: "a", default: true });
    expect(seen).toEqual(["a"]);
  });
});
