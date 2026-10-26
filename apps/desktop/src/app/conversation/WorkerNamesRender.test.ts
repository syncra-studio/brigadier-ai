import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

test("a worker @-mentioned reads as its name wherever the user sees it, its `task-N` only in what is sent", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "worker-names.html", "worker-names-result", 60000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined, String(seen.error));
  assert.equal(seen.draft, "ask Build the importer now", "the kept draft's chip");
  assert.deepEqual(seen.queued, { text: "then @Write the docs checks it", title: "then @Write the docs checks it" }, "the queued message above the composer");
  assert.equal(seen.sentNotice, "Sent to Build the importer: @Build the importer rebase first", "the notice of one sent on to a worker");
  assert.equal(seen.bubble, "Build the importer", "the sent bubble's chip");
  assert.equal(seen.railPreview, "@Build the importer look at the parser", "the rail's preview");
  assert.equal(seen.railLabel, "@Build the importer look at the parser", "the rail marker's label");
  // Under Workers with their state, then under Message directly.
  assert.deepEqual(seen.menu, ["Build the importerrunning", "Write the docsrunning", "Build the importer", "Write the docs"], "the @ menu");
  assert.equal(seen.pulled, "then Write the docs checks it", "a queued message taken back into the composer");
  assert.deepEqual(seen.anywhere, [], "no task-N anywhere on screen");
});
