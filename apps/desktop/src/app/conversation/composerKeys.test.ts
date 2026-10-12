import assert from "node:assert/strict";
import { test } from "node:test";

import { enterAction, sendLane } from "@/app/conversation/composerKeys";
import { parseStroke, shortcutsFor } from "@/app/shortcutCatalog";
import type { FollowUp, SendShortcut } from "@/ipc/generated";

const enter = (mods: { ctrl?: boolean; alt?: boolean; shift?: boolean; cmd?: boolean } = {}) => ({
  ctrlKey: mods.ctrl ?? false,
  altKey: mods.alt ?? false,
  shiftKey: mods.shift ?? false,
  metaKey: mods.cmd ?? false,
});

test("Enter sends, ⌘Enter sends the other way and Shift+Enter is left to the editor", () => {
  assert.equal(enterAction(enter(), true, "enter"), "send");
  assert.equal(enterAction(enter({ cmd: true }), true, "enter"), "other");
  assert.equal(enterAction(enter({ shift: true }), true, "enter"), null);
  // Off macOS Ctrl is the modifier, and the Windows key isn't.
  assert.equal(enterAction(enter({ ctrl: true }), false, "enter"), "other");
  assert.equal(enterAction(enter({ cmd: true }), false, "enter"), null);
  assert.equal(enterAction(enter({ ctrl: true }), true, "enter"), null);
  assert.equal(enterAction(enter({ alt: true }), true, "enter"), null);
});

test("with ⌘Enter chosen, Enter starts a new line and ⇧⌘Enter sends the other way", () => {
  assert.equal(enterAction(enter(), true, "modEnter"), null);
  assert.equal(enterAction(enter({ shift: true }), true, "modEnter"), null);
  assert.equal(enterAction(enter({ cmd: true }), true, "modEnter"), "send");
  assert.equal(enterAction(enter({ cmd: true, shift: true }), true, "modEnter"), "other");
  assert.equal(enterAction(enter({ ctrl: true }), false, "modEnter"), "send");
  assert.equal(enterAction(enter({ ctrl: true, shift: true }), false, "modEnter"), "other");
});

test("a message sent while an answer works goes the default way unless the other key says", () => {
  const free = { pulled: false };
  assert.equal(sendLane("followUp", { ...free, followUp: "queue" }), "auto");
  assert.equal(sendLane("followUp", { ...free, followUp: "steer" }), "steer");
  // The other key: steer when queueing is the default, queue (the daemon's own way) when not.
  assert.equal(sendLane("steer", { ...free, followUp: "queue" }), "steer");
  assert.equal(sendLane("auto", { ...free, followUp: "steer" }), "auto");
  // A queued message pulled out to edit goes back to its slot, unless steered in explicitly.
  assert.equal(sendLane("followUp", { pulled: true, followUp: "steer" }), "queue");
  assert.equal(sendLane("auto", { pulled: true, followUp: "queue" }), "queue");
  assert.equal(sendLane("steer", { pulled: true, followUp: "queue" }), "steer");
});

test("the shortcuts page shows the composer keys the composer acts on, on both platforms", () => {
  const actions = { send: "send", "follow-up-other": "other" } as const;
  for (const mac of [true, false]) {
    for (const sendShortcut of ["enter", "modEnter"] as SendShortcut[]) {
      for (const followUp of ["queue", "steer"] as FollowUp[]) {
        const shortcuts = shortcutsFor(mac, true, { sendShortcut, followUp });
        const keysOf = (id: string) => shortcuts.find((s) => s.id === id)?.keys ?? [];
        const pressOf = (chip: string) => {
          const stroke = parseStroke(chip);
          return enter({
            shift: stroke.shift,
            alt: stroke.alt,
            ctrl: stroke.ctrl || (!mac && stroke.cmd),
            cmd: mac && stroke.cmd,
          });
        };
        const where = `${sendShortcut}, ${followUp}, ${mac ? "macOS" : "other"}`;
        for (const [id, action] of Object.entries(actions)) {
          const [chip = ""] = keysOf(id);
          assert.equal(enterAction(pressOf(chip), mac, sendShortcut), action, `${id}: ${where}`);
        }
        for (const chip of keysOf("new-line")) {
          assert.equal(enterAction(pressOf(chip), mac, sendShortcut), null, `new line ${chip}: ${where}`);
        }
        const other = shortcuts.find((s) => s.id === "follow-up-other");
        assert.equal(other?.name, followUp === "queue" ? "Steer" : "Queue", where);
      }
    }
  }
});
