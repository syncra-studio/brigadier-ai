import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";
import { trackPageHidden, type VisibilityDocument } from "@/lib/pageHidden";

function fakeDocument(hidden: boolean) {
  const listeners: (() => void)[] = [];
  const doc = {
    hidden,
    documentElement: { dataset: {} as DOMStringMap },
    addEventListener: (_type: "visibilitychange", listener: () => void) => listeners.push(listener),
  } satisfies VisibilityDocument;
  const change = (next: boolean) => {
    doc.hidden = next;
    for (const listener of listeners) listener();
  };
  return { doc, change };
}

test("a page that starts hidden is marked before anything can close", () => {
  const { doc } = fakeDocument(true);
  trackPageHidden(doc);
  assert.equal(doc.documentElement.dataset.pageHidden, "");
});

test("the mark follows the page as it is covered and shown again", () => {
  const { doc, change } = fakeDocument(false);
  trackPageHidden(doc);
  assert.equal("pageHidden" in doc.documentElement.dataset, false);
  change(true);
  assert.equal(doc.documentElement.dataset.pageHidden, "");
  change(false);
  assert.equal("pageHidden" in doc.documentElement.dataset, false);
});

test("a dialog or popover closed while the page is hidden leaves though no frame runs", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "hidden-exit.html?drive=1", "hidden-exit-result")) as Record<
    string,
    { marked: boolean; mounted: string[] }
  >;
  const dialog = ['[data-slot="dialog-content"]', '[data-slot="dialog-overlay"]'];
  const popover = ['[data-slot="popover-content"]'];
  assert.deepEqual(seen, {
    // Shown: the exit animation plays and ends, as before.
    "popover shown": { marked: false, mounted: [] },
    "dialog shown": { marked: false, mounted: [] },
    // Hidden with no frames: gone at once.
    "popover hidden": { marked: true, mounted: [] },
    "dialog hidden": { marked: true, mounted: [] },
    // The control: visible with no frames, the exit never ends, as the covered window did.
    "popover frozen": { marked: false, mounted: popover },
    "dialog frozen": { marked: false, mounted: dialog },
  });
});
