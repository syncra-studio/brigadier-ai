import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

test("a project session's composer starts a message as a new background session: the Send tooltip names ⌘⏎, the arrow turns while ⌘ is held, ⌘-click and ⌘⏎ start one, and / sets a New session chip for the next send", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "composer-background.html", "composer-background-result", 90000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined, String(seen.error));
  assert.equal(seen.tooltip, "Send ⏎ · Start as a new session ⌘⏎");
  assert.equal(seen.arrowBefore, false);
  assert.equal(seen.heldTooltip, "Start as a new session");
  assert.equal(seen.commandClick, "new one");
  // A plain click sends here, even after a ⌘-click whose send never came.
  assert.equal(seen.click, "here two");
  assert.equal(seen.commandEnter, "new three");
  // The chip: its one send goes to a new session and it goes; cleared, the next send stays here.
  assert.equal(seen.chipSend, "new four");
  assert.equal(seen.chipAfter, false);
  assert.equal(seen.afterChip, "here five");
});
