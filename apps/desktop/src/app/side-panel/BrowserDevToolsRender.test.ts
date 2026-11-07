import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

// A Browser tab's developer tools in Chromium over a faked shell (Electron's main process isn't here): the dev button's
// menu, DevTools' pane with its dock, the device bar with rotation, and the picker's strips and toast.
test("browser dev tools: the button's menu, the dock, the device bar, the picker", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "browser-dev-tools.html?drive=1", "browser-dev-tools-result", 60000)) as Record<string, unknown>;
  if (process.env.BROWSER_DEV_SEEN) console.log(JSON.stringify(seen, null, 2));
  assert.equal(seen.error, undefined);

  assert.deepEqual(seen.menu, ["Pick an element", "DevTools", "Device sizes"]);
  assert.equal(seen.opened, 1);
  assert.equal(seen.aboveComposer, true, "the pane stops above the floating composer");
  assert.deepEqual(seen.menuOn, ["Pick an element", "DevTools", "Device sizes", "Dock DevTools below"]);
  assert.equal(seen.menuHides, true, "a menu over the pane hides it");
  assert.equal(seen.dock, "bottom");
  assert.equal(seen.belowIsWide, true);
  assert.equal(seen.separator, "horizontal");
  assert.equal(seen.menuBelow, "Dock DevTools beside");
  assert.equal(seen.dockBack, "right");
  assert.equal(seen.dialogHides, true, "a dialog hides the pane");
  assert.equal(seen.dialogGone, true, "and it comes back");
  assert.equal(seen.closed, 1);

  const first = seen.firstDevice as { width: number; height: number };
  assert.deepEqual(seen.turned, { width: first.height, height: first.width });
  assert.deepEqual(seen.fields, ["Width", "Height"]);
  assert.ok((seen.options as number) >= 9);
  assert.equal(seen.leftBar, true);
  assert.equal(seen.cleared, 1);

  assert.equal(seen.picking, "Click an element of the page. Esc leaves.Stop");
  assert.deepEqual(seen.stopped, { strip: false, asked: 1 });
  assert.match(String(seen.picked), /button.*#buy.*100×30/);
  assert.deepEqual(seen.noChat, ["There is no chat open to add it to."]);
  assert.equal(seen.stillPicked, true);
});
