import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

// #663: a question card's words can be selected and copied, and a command an answer names has a
// copy button of its own, beside the answer: copying it answers nothing.
test("a question card's words select, and an answer's command copies without answering", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "question-card-copy.html", "question-card-copy-result", 60000)) as {
    error?: string;
    selected: string;
    copyButtons: number;
    copyInsideAnswer: boolean;
    copied: string[];
    answered: boolean;
    stillAsks: boolean;
  };
  assert.equal(seen.error, undefined, seen.error ?? "");
  assert.equal(seen.selected, "Clean up the leftover brigadier-* folders in TEMP now?");
  // Only the answer that names a command has one, outside its answer button.
  assert.equal(seen.copyButtons, 1);
  assert.equal(seen.copyInsideAnswer, false);
  assert.deepEqual(seen.copied, ["Get-ChildItem -Path $env:TEMP -Filter 'brigadier-*' -Recurse | Remove-Item -Recurse -Force"]);
  assert.equal(seen.answered, false, "copying sent no answer");
  assert.equal(seen.stillAsks, true, "the question still waits");
});
