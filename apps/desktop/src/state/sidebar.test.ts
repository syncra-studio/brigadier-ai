import assert from "node:assert/strict";
import { test } from "node:test";

import {
  cachedCollapseMode,
  cachedOpen,
  COLLAPSED_KEY,
  COLLAPSE_MODE_KEY,
  saveCollapseMode,
  saveOpen,
} from "./sidebar";

test("sidebar open state and collapse mode persist independently across reloads", () => {
  const data = new Map<string, string>();
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (key: string) => data.get(key) ?? null,
      setItem: (key: string, value: string) => data.set(key, value),
      removeItem: (key: string) => data.delete(key),
    },
  });
  try {
    assert.equal(cachedOpen(), true);
    assert.equal(cachedCollapseMode(), "strip");
    saveCollapseMode("hidden");
    assert.equal(cachedOpen(), true);
    saveOpen(false);
    assert.equal(data.get(COLLAPSED_KEY), "1");
    assert.equal(data.get(COLLAPSE_MODE_KEY), "hidden");
    assert.equal(cachedOpen(), false);
    assert.equal(cachedCollapseMode(), "hidden");
    saveOpen(true);
    assert.equal(data.has(COLLAPSED_KEY), false);
    assert.equal(cachedOpen(), true);
    assert.equal(cachedCollapseMode(), "hidden");
    saveCollapseMode("strip");
    assert.equal(cachedCollapseMode(), "strip");
    data.set(COLLAPSE_MODE_KEY, "invalid");
    data.set(COLLAPSED_KEY, "invalid");
    assert.equal(cachedCollapseMode(), "strip");
    assert.equal(cachedOpen(), true);
  } finally {
    delete (globalThis as { localStorage?: Storage }).localStorage;
  }
});

test("unavailable sidebar storage falls back safely and never breaks toggling", () => {
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    get: () => {
      throw new Error("disabled");
    },
  });
  try {
    assert.equal(cachedOpen(), true);
    assert.equal(cachedCollapseMode(), "strip");
    assert.doesNotThrow(() => saveOpen(false));
    assert.doesNotThrow(() => saveOpen(true));
    assert.doesNotThrow(() => saveCollapseMode("hidden"));
  } finally {
    delete (globalThis as { localStorage?: Storage }).localStorage;
  }
});
