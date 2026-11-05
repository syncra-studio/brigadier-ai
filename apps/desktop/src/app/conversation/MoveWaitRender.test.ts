import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("a switch waiting for background jobs says so on the composer, and Switch now warns first", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "move-wait.html", "move-wait-result", 90000));
  assert.equal(seen.error, undefined, String(seen.error));
  assert.equal(seen.row, "Switching to Codex once background jobs endSwitch now");
  assert.match(seen.warning, /Switch to Codex now\?/);
  assert.match(seen.warning, /may end them/);
  assert.equal(seen.askedBeforeAnswer, 0);
  assert.equal(seen.askedAfterKeepWaiting, 0);
  assert.deepEqual(seen.asked, ["move-wait"]);
  assert.equal(seen.rowAfter, "Switching to Codex…");
  assert.equal(seen.buttonAfter, false);
});
