import assert from "node:assert/strict";
import { test } from "node:test";

import { leftWords } from "@/app/conversation/undoWords";

const held = [
  { path: "README.md", reason: "A command changed README.md, and Brigadier can't tell that change from yours, so Undo leaves it as it is." },
  { path: "src/dates.ts", reason: "src/dates.ts also changed during this request in a way Brigadier didn't see (by you, or by one of the thread's own tools), so Undo leaves it as it is." },
];

test("what an Undo or Reapply left: one file says why, several are named, none says nothing", () => {
  assert.equal(leftWords(held), ". Left 2 files as they are (README.md, dates.ts): changed during the request by you, or by the thread's own commands and tools");
  assert.equal(leftWords(held, true), ". 2 files stay as they are (README.md, dates.ts): Undo left them");
  assert.equal(leftWords([]), "");
  assert.equal(leftWords(held.slice(0, 1)), `. ${held[0]!.reason}`);
  assert.equal(leftWords(held.slice(1), true), ". dates.ts stays as it is: Undo left it");
});
