import assert from "node:assert/strict";
import { test } from "node:test";

import { type BrowserKey, browserKey, stepZoom, zoomLabel } from "@/app/conversation/browserKeys";
import { MENU_KEYS } from "@/app/paneShortcuts";
import { findLabel, NO_FIND } from "@/state/browserFind";

type Mods = { ctrl?: boolean; alt?: boolean; shift?: boolean; cmd?: boolean };
const press = (code: string, mods: Mods = {}) => ({
  code,
  ctrlKey: mods.ctrl ?? false,
  altKey: mods.alt ?? false,
  shiftKey: mods.shift ?? false,
  metaKey: mods.cmd ?? false,
});

test("the Browser menu's items reach the tab as the keys it acts on", () => {
  // As `usePaneShortcuts` sends a menu item on: ⌘ unless the item is a Control one.
  const expected: Record<string, BrowserKey> = {
    address: "address",
    "page-back": "back",
    "page-forward": "forward",
    reload: "reload",
    "force-reload": "force-reload",
    find: "find",
    "find-next": "find-next",
    "find-previous": "find-previous",
    "zoom-in": "zoom-in",
    "zoom-out": "zoom-out",
    "zoom-reset": "zoom-reset",
  };
  for (const [item, key] of Object.entries(expected)) {
    const menu = MENU_KEYS[item];
    assert.ok(menu, item);
    const sent = press(menu.code, { cmd: !menu.control, ctrl: menu.control ?? false, shift: menu.shift ?? false, alt: menu.alt ?? false });
    assert.equal(browserKey(sent, true), key, item);
  }
});

test("the tab's other menu items are none of its keys", () => {
  for (const item of ["new", "reopen", "close", "close-others", "previous-tab", "next-tab", "previous", "next", "back", "forward", "tab-1", "review", "sidebar"]) {
    const menu = MENU_KEYS[item];
    assert.ok(menu, item);
    const sent = press(menu.code, { cmd: !menu.control, ctrl: menu.control ?? false, shift: menu.shift ?? false, alt: menu.alt ?? false });
    assert.equal(browserKey(sent, true), null, item);
  }
});

test("browser keys on macOS: ⌘ alone or with Shift, never Ctrl or Option", () => {
  assert.equal(browserKey(press("Equal", { cmd: true, shift: true }), true), "zoom-in", "⌘+");
  assert.equal(browserKey(press("NumpadAdd", { cmd: true }), true), "zoom-in");
  assert.equal(browserKey(press("NumpadSubtract", { cmd: true }), true), "zoom-out");
  assert.equal(browserKey(press("KeyR", { cmd: true, alt: true }), true), null, "⌥⌘R renames the conversation");
  assert.equal(browserKey(press("ArrowLeft", { cmd: true, alt: true }), true), null, "⌥⌘← is the previous tab");
  assert.equal(browserKey(press("ArrowLeft", { cmd: true, shift: true }), true), null, "⇧⌘← selects to the line's start");
  assert.equal(browserKey(press("KeyF", { ctrl: true }), true), null);
  assert.equal(browserKey(press("KeyL", { cmd: true, shift: true }), true), null);
  assert.equal(browserKey(press("KeyF"), true), null);
  assert.equal(browserKey(press("ArrowLeft", { alt: true }), true), null, "⌥← moves by word on macOS");
});

test("browser keys elsewhere: Ctrl for ⌘, and Alt+← / Alt+→ back and forward", () => {
  assert.equal(browserKey(press("KeyR", { ctrl: true }), false), "reload");
  assert.equal(browserKey(press("KeyR", { ctrl: true, shift: true }), false), "force-reload");
  assert.equal(browserKey(press("KeyF", { ctrl: true }), false), "find");
  assert.equal(browserKey(press("KeyG", { ctrl: true, shift: true }), false), "find-previous");
  assert.equal(browserKey(press("Digit0", { ctrl: true }), false), "zoom-reset");
  assert.equal(browserKey(press("ArrowLeft", { alt: true }), false), "back");
  assert.equal(browserKey(press("ArrowRight", { alt: true }), false), "forward");
  assert.equal(browserKey(press("ArrowLeft", { ctrl: true }), false), null, "Ctrl+← moves by word");
  assert.equal(browserKey(press("KeyR", { cmd: true }), false), null);
});

test("zoom steps 10% between 30% and 300%", () => {
  assert.equal(stepZoom(1, 1), 1.1);
  assert.equal(stepZoom(1.1, 1), 1.2);
  assert.equal(stepZoom(1, -1), 0.9);
  assert.equal(stepZoom(0.3, -1), 0.3);
  assert.equal(stepZoom(3, 1), 3);
  assert.equal(stepZoom(2.9, 1), 3);
  // A zoom off the steps lands on the nearest one past it.
  assert.equal(stepZoom(1.04, 1), 1.1);
  assert.equal(zoomLabel(1.1), "110%");
  assert.equal(zoomLabel(0.3), "30%");
  assert.equal(zoomLabel(3), "300%");
});

test("the find bar says which match of how many, or that there is none", () => {
  const find = { ...NO_FIND, open: true, query: "foo" };
  assert.equal(findLabel({ ...NO_FIND, open: true }), "");
  assert.equal(findLabel({ ...find, searching: true }), "");
  assert.equal(findLabel(find), "No results");
  assert.equal(findLabel({ ...find, found: true, current: 2, total: 5 }), "2 of 5");
  assert.equal(findLabel({ ...find, found: true, current: 0, total: 5 }), "1 of 5");
  assert.equal(findLabel({ ...find, found: true, current: 7, total: 1000 }), "7 of 1000+");
  assert.equal(findLabel({ ...find, found: true }), "Found");
});
