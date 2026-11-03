import assert from "node:assert/strict";
import { test } from "node:test";

import { heldMap } from "./held";

const store = new Map<string, string>();
Object.assign(globalThis, {
  localStorage: { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => store.set(k, v) },
});
const isNumber = (v: unknown): v is number => typeof v === "number";

test("a held value is there after a restart, newest kept, a bad one dropped", () => {
  const first = heldMap("t.held", 2, isNumber);
  first.set("a", 1);
  first.set("b", 2);
  first.set("c", 3);
  first.set("b", undefined);
  const raw = JSON.parse(store.get("t.held") ?? "{}") as Record<string, unknown>;
  store.set("t.held", JSON.stringify({ ...raw, bad: "x" }));
  const again = heldMap("t.held", 2, isNumber);
  assert.equal(again.get("a"), undefined);
  assert.equal(again.get("b"), undefined);
  assert.equal(again.get("c"), 3);
  assert.equal(again.get("bad"), undefined);
});
