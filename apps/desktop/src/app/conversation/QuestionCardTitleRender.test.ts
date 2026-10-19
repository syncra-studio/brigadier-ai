import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

// An interview's round on the question card (Q78): its title sits over the question, the
// recommended option is marked.
test("a titled round shows its title over the question", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "question-card-title.html", "question-card-title-result", 60000)) as {
    error?: string;
    kind: string;
    question: string;
    kindAbove: boolean;
    cardText: string;
  };
  assert.equal(seen.error, undefined, seen.error ?? "");
  assert.equal(seen.kind, "Round 1 · Location");
  assert.match(seen.question, /^Rent near the central market is about 9 EUR\/m²/);
  assert.equal(seen.kindAbove, true, "the title is over the question");
  assert.match(seen.cardText, /Recommended/);
});
