import assert from "node:assert/strict";
import { test } from "node:test";

import { codeParts } from "@/app/conversation/cards/questionRound";

test("a card line shows its backticked names as code, without the backticks", () => {
  assert.deepEqual(codeParts("Merge `brigadier/c/session` into `main`?"), [
    { text: "Merge ", code: false },
    { text: "brigadier/c/session", code: true },
    { text: " into ", code: false },
    { text: "main", code: true },
    { text: "?", code: false },
  ]);
  assert.deepEqual(codeParts("Which format?"), [{ text: "Which format?", code: false }]);
  assert.deepEqual(codeParts("An unclosed ` stays"), [{ text: "An unclosed ` stays", code: false }]);
});
