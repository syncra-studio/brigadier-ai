import assert from "node:assert/strict";
import { test } from "node:test";

import { undoToast } from "@/app/conversation/undoWords";

const held = [
  { path: "README.md", reason: "A command changed README.md, and Brigadier can't tell that change from yours, so Undo leaves it as it is." },
  { path: "src/dates.ts", reason: "You changed src/dates.ts during this request too, so Undo leaves it as it is." },
];

test("an Undo or Reapply that put nothing back says so, not that changes were reverted", () => {
  assert.equal(
    undoToast(held, 0, false),
    "Nothing was reverted. Left 2 files as they are (README.md, dates.ts): changed by you or by a command during the request",
  );
  assert.equal(undoToast(held, 0, true), "Nothing was reapplied. 2 files stay as they are (README.md, dates.ts): Undo left them");
  assert.equal(undoToast([], 0, false), "Nothing was reverted");
});

test("an Undo or Reapply that put something back says what it did, then what it left", () => {
  assert.equal(undoToast([], 2, false), "Changes reverted");
  assert.equal(undoToast(held.slice(0, 1), 1, false), `Changes reverted. ${held[0]!.reason}`);
  assert.equal(undoToast(held.slice(1), 1, true), "Changes reapplied. dates.ts stays as it is: Undo left it");
});
