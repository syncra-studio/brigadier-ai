import assert from "node:assert/strict";
import { test } from "node:test";

test("the splash stops waiting for its fade once the page is hidden, as no frame will end it", async () => {
  const listeners: (() => void)[] = [];
  let removed = false;
  const never = new Promise<void>(() => {});
  const element = {
    getAnimations: () => [{ finished: never }],
    remove: () => {
      removed = true;
    },
  };
  const doc = {
    hidden: false,
    documentElement: { dataset: {} as DOMStringMap, hasAttribute: () => false },
    getElementById: (id: string) => (id === "splash" ? element : null),
    addEventListener: (_type: string, listener: () => void) => listeners.push(listener),
  };
  Object.assign(globalThis, { document: doc, getComputedStyle: () => ({ opacity: "1" }) });
  const { revealApp } = await import("@/lib/splash");

  const revealing = revealApp();
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.equal(removed, false, "a shown page waits for its fade");

  doc.hidden = true;
  for (const listener of listeners) listener();
  await revealing;
  assert.equal(removed, true);
  assert.equal("splash" in doc.documentElement.dataset, false);
});
