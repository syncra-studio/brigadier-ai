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

test("a card that comes while the page is hidden is simply there when it shows again", { timeout: 60000 }, async (t) => {
  type Card = { animations: number; opacity: string; height: number } | null;
  const seen = JSON.parse(await renderFixtureLive(t, "hidden-entrance.html?drive=1", "hidden-entrance-result")) as Record<"hidden" | "shown", { slot: Card; rail: Card; footer: Card; options: Card }>;
  // Hidden with no frames: no entrance, so it shows whole the moment the window does. So do a
  // decided approval's footer in the thread and a question card's options.
  for (const card of [seen.hidden.slot, seen.hidden.rail, seen.hidden.footer, seen.hidden.options]) {
    assert.ok(card, "the card mounted");
    assert.equal(card.animations, 0);
    assert.equal(card.opacity, "1");
    assert.ok(card.height > 0);
  }
  // The control: shown, each still opens with its animation; held at its first frame, as the
  // covered window held it, the approval is invisible though it takes its room.
  assert.equal(seen.shown.slot?.animations, 1);
  assert.equal(seen.shown.slot?.opacity, "0");
  assert.ok((seen.shown.slot?.height ?? 0) > 0);
  assert.equal(seen.shown.rail?.animations, 1);
  assert.equal(seen.shown.footer?.animations, 1);
  assert.ok((seen.shown.options?.animations ?? 0) >= 1);
});
