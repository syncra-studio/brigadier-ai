import assert from "node:assert/strict";
import { test } from "node:test";

import { changedPaneSize, PANE_SIZE_KEY, savedPaneSizes } from "./paneSizes";

test("pane dimensions survive reload independently and reset one at a time", () => {
  const data = new Map<string, string>();
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (key: string) => data.get(key) ?? null,
      setItem: (key: string, value: string) => data.set(key, value),
    },
  });
  let sizes = changedPaneSize({}, "browser", 510);
  sizes = changedPaneSize(sizes, "workers", 390);
  changedPaneSize(sizes, "terminal", 310);
  assert.deepEqual(savedPaneSizes(), {
    browser: 510,
    workers: 390,
    terminal: 310,
  });
  changedPaneSize(savedPaneSizes(), "browser", null);
  assert.deepEqual(savedPaneSizes(), { workers: 390, terminal: 310 });
  data.set(
    PANE_SIZE_KEY,
    '{"browser":-1,"workers":"400","terminal":260,"unknown":300}',
  );
  assert.deepEqual(savedPaneSizes(), { terminal: 260 });
  data.set(PANE_SIZE_KEY, "broken");
  assert.deepEqual(savedPaneSizes(), {});
});

test("unavailable storage keeps other pane sizes in memory", () => {
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    get: () => {
      throw new Error("disabled");
    },
  });
  assert.deepEqual(savedPaneSizes(), {});
  assert.deepEqual(changedPaneSize({ workers: 400 }, "terminal", 280), {
    workers: 400,
    terminal: 280,
  });
  delete (globalThis as { localStorage?: Storage }).localStorage;
});
