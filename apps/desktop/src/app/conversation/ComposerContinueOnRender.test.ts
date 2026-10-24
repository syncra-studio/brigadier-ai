import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("a session's model picker continues on another agent from its models, and takes the keyboard (S38c)", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "composer-continue-on.html", "composer-continue-on-result", 90000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined, String(seen.error));
  // Its own agent's list has only models, and no line.
  assert.deepEqual(seen.claude, { line: null, models: ["Opus 5.5Default"] });
  // The other agent's list says a pick continues there; picking its model moves the session on it.
  const line = "Picking a model continues on Codex: fresh context, with a hand-off note.";
  assert.deepEqual(seen.codex, { line, models: ["GPT-5.5"] });
  assert.deepEqual(seen.asked, [{ provider: "codex", model: { provider: "codex", model: "gpt-5.5", effort: "medium" } }]);
  // ↓ from the search to the picked model, ← to its agent, ↓ browses the next, → into its models,
  // typing searches, the first Escape clears the search and keeps the picker open.
  assert.deepEqual(seen.keys, [
    ["ArrowDown", "Opus 5.5Default"],
    ["ArrowLeft", "Claude Code"],
    ["ArrowDown", "Codex"],
    ["ArrowRight", "GPT-5.5"],
    ["g", "search:g"],
    ["Escape", "search:"],
  ]);
  assert.equal(seen.browsingLine, line);
  assert.deepEqual(seen.searched, ["GPT-5.5Codex"]);
  assert.equal(seen.openAfterEscape, true);
});
