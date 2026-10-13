import assert from "node:assert/strict";
import { test } from "node:test";
import { clipPath, modalOpen } from "./browserLayer";

/** A layer as `modalOpen` reads it: what it matches and what it holds. */
const layer = (selectors: string[], holds: string[] = []) =>
  ({
    matches: (selector: string) => selectors.some((each) => selector.split(", ").includes(each)),
    querySelector: (selector: string) => (holds.some((each) => selector.split(", ").includes(each)) ? {} : null),
  }) as unknown as Element;

test("the app's clip is the window with each page's hole cut the other way round", () => {
  assert.equal(clipPath(800, 600, []), 'path(nonzero, "M0 0H800V600H0Z")');
  assert.equal(
    clipPath(800, 600, [
      { x: 10, y: 20, width: 300, height: 200 },
      { x: 100, y: 50, width: 50, height: 60 },
    ]),
    'path(nonzero, "M0 0H800V600H0ZM10 20V220H310V20ZM100 50V110H150V50Z")',
  );
});

test("menus, popovers and dialogs take clicks over a page; tooltips don't", () => {
  const tooltip = layer(["[data-radix-popper-content-wrapper]"], ["[data-slot=tooltip-content]"]);
  const menu = layer(["[data-radix-popper-content-wrapper]"], ["[role=menu]"]);
  const dialog = layer(["[role=dialog]"]);
  assert.equal(modalOpen([]), false);
  assert.equal(modalOpen([tooltip]), false);
  assert.equal(modalOpen([layer(["[data-radix-popper-content-wrapper]"], ["[role=tooltip]"])]), false);
  assert.equal(modalOpen([tooltip, menu]), true);
  assert.equal(modalOpen([dialog]), true);
});
