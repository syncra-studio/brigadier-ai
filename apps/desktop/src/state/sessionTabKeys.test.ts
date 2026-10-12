import assert from "node:assert/strict";
import { test } from "node:test";

import { sessionTabKey } from "./sessionTabKeys";

type Mods = { cmd?: boolean; ctrl?: boolean; alt?: boolean; shift?: boolean };
/** A key press with "⌘" meaning Command on macOS and Ctrl elsewhere. */
const press = (code: string, mods: Mods, mac: boolean) => ({
  code,
  metaKey: mac && (mods.cmd ?? false),
  ctrlKey: (mods.ctrl ?? false) || (!mac && (mods.cmd ?? false)),
  altKey: mods.alt ?? false,
  shiftKey: mods.shift ?? false,
  isComposing: false,
});

test("every tab key does its job on both platforms", () => {
  const cases: [string, Mods, ReturnType<typeof sessionTabKey>][] = [
    ["KeyT", { cmd: true }, { type: "new", kind: "newTab" }],
    ["KeyW", { cmd: true }, { type: "close" }],
    ["KeyW", { cmd: true, alt: true }, { type: "closeOthers" }],
    ["Tab", { ctrl: true }, { type: "step", step: 1 }],
    ["Tab", { ctrl: true, shift: true }, { type: "step", step: -1 }],
    ["BracketRight", { cmd: true, shift: true }, { type: "step", step: 1 }],
    ["BracketLeft", { cmd: true, shift: true }, { type: "step", step: -1 }],
    ["ArrowRight", { cmd: true, alt: true }, { type: "step", step: 1 }],
    ["ArrowLeft", { cmd: true, alt: true }, { type: "step", step: -1 }],
    ["Digit1", { cmd: true }, { type: "number", number: 1 }],
    ["Digit9", { cmd: true }, { type: "number", number: 9 }],
  ];
  for (const mac of [true, false]) {
    for (const [code, mods, action] of cases) {
      assert.deepEqual(sessionTabKey(press(code, mods, mac), mac), action, `${code} ${JSON.stringify(mods)} on ${mac ? "macOS" : "other"}`);
    }
  }
});

test("near misses do nothing: arrows without both modifiers, extra Shift, Delete, composition", () => {
  for (const mac of [true, false]) {
    assert.equal(sessionTabKey(press("ArrowLeft", { cmd: true }, mac), mac), null, "⌘← is the browser's back");
    assert.equal(sessionTabKey(press("ArrowLeft", { alt: true }, mac), mac), null, "⌥← moves by word");
    assert.equal(sessionTabKey(press("ArrowRight", { cmd: true, alt: true, shift: true }, mac), mac), null);
    assert.equal(sessionTabKey(press("KeyW", { cmd: true, alt: true, shift: true }, mac), mac), null);
    assert.equal(sessionTabKey(press("Backspace", {}, mac), mac), null, "Delete is the tab strip's own");
    assert.equal(sessionTabKey(press("Delete", {}, mac), mac), null);
    assert.equal(sessionTabKey({ ...press("KeyW", { cmd: true, alt: true }, mac), isComposing: true }, mac), null);
  }
  assert.equal(sessionTabKey({ ...press("KeyW", { ctrl: true, alt: true }, true) }, true), null, "Ctrl isn't Command on macOS");
});
