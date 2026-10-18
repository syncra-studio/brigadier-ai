import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("the permission menu opens from the / menu's Permissions command and from ↑ on the focused pill; Model opens its picker the same way (issue 95)", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "composer-permissions.html", "composer-permissions-result", 90000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined, String(seen.error));
  assert.equal(seen.command, "Permissions | Approve for me · change how actions are approved");
  assert.equal(seen.commandIcon, true);
  assert.equal(seen.fromSlash, "open");
  assert.equal(seen.stillOpen, "open");
  assert.equal(seen.model, "open");
  assert.equal(seen.fromArrowUp, "open");
});
