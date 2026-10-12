import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("a message sent from a page shows its title without granting pages by their typed titles", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "page-message.html", "page-message-result"));
  assert.deepEqual(seen.labels, ["From page: Launch plan <draft>"]);
  assert.deepEqual(seen.text, ["From page: Launch plan <draft>Tick the first item.", "A plain follow-up."]);
  assert.deepEqual(seen.inferred, []);
  assert.deepEqual(seen.remembered, []);
  assert.deepEqual(seen.kept, [{ type: "page", id: "page-one", title: "Launch plan <draft>" }]);
});
