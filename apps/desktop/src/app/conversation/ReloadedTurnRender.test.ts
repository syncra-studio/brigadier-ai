import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

type Seen = { turns: number; failed: string | null; answer: string | null; text: string | null; page: string | null };

// Harness D49/D58: after a restart, the Codex turn the Stop hook sent back once, which then asked
// on the card and ended in an empty reply, draws: an empty reply is no part of it.
test("a reloaded Codex turn that ends in an empty reply renders, its last words the answer", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "reloaded-turn.html", "reloaded-turn-result", 15000)) as Seen;
  assert.equal(seen.failed, null, `the turn failed to render: ${seen.failed}`);
  assert.equal(seen.turns, 1, seen.page ?? "");
  assert.equal(seen.answer, "Putting them on the card.");
});
