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
  const sizes = changedPaneSize({}, "review", 510);
  changedPaneSize(sizes, "terminal", 310);
  assert.deepEqual(savedPaneSizes(), {
    review: 510,
    terminal: 310,
  });
  changedPaneSize(savedPaneSizes(), "review", null);
  assert.deepEqual(savedPaneSizes(), { terminal: 310 });
  data.set(
    PANE_SIZE_KEY,
    '{"browser":-1,"sideChat":"400","terminal":260,"workers":390,"files":300,"source":300,"unknown":300}',
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
  assert.deepEqual(changedPaneSize({ review: 400 }, "terminal", 280), {
    review: 400,
    terminal: 280,
  });
  delete (globalThis as { localStorage?: Storage }).localStorage;
});
