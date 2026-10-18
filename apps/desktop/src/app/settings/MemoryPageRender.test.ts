import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

type Row = { text: string; line: string };

// The real page in Chromium over a faked daemon that keeps the memories: Settings › Memory lists
// what Brigadier remembers with when and where each was learned; Edit saves in place, a refused
// edit says why and stays open, Cancel leaves it as it was, Delete removes it at once.
test("Memory lists what Brigadier remembers, and edits and deletes it", { timeout: 60000 }, async (t) => {
  const dump = await renderFixtureLive(t, "memory.html?drive=1", "memory-result", 30000);
  const seen = JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&")) as {
    title: string;
    description: string;
    first: Row[];
    editing: { value: string; buttons: string[] };
    edited: Row[];
    refused: { error: string; stillEditing: boolean };
    cancelled: Row[];
    deleted: Row[];
    calls: string[];
  };
  assert.equal(seen.title, "Memory");
  assert.match(seen.description, /where you live, your work, your tools, what you prefer/);
  assert.match(seen.description, /Passwords, card numbers, health and money details are never kept/);
  assert.deepEqual(
    seen.first.map((row) => row.text),
    ["Lives in Chisinau, Moldova", "Works as an electrician", "Prints on a Bambu Lab P1S with a hardened steel nozzle"],
  );
  assert.match(seen.first[0]!.line, /from the chat “Filament shops”$/);
  assert.match(seen.first[2]!.line, /from you$/);
  assert.equal(seen.editing.value, "Lives in Chisinau, Moldova");
  assert.deepEqual(seen.editing.buttons.slice(0, 2), ["Cancel", "Save"]);
  assert.equal(seen.edited[0]!.text, "Lives in Iasi, Romania");
  assert.deepEqual(seen.refused, { error: "Brigadier doesn't keep that: it holds a card number", stillEditing: true });
  assert.equal(seen.cancelled[1]!.text, "Works as an electrician");
  assert.deepEqual(
    seen.deleted.map((row) => row.text),
    ["Lives in Iasi, Romania", "Prints on a Bambu Lab P1S with a hardened steel nozzle"],
  );
  assert.deepEqual(seen.calls, [
    "edit m1 Lives in Iasi, Romania",
    "edit m2 Card 4111 1111 1111 1111",
    "forget m2",
  ]);
});
