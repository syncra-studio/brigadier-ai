import assert from "node:assert/strict";
import { test } from "node:test";

import { cliRunOutput, ownRunOutput } from "@/app/conversation/activity/ownRun";

test("an own command's [exit N] line is its exit code, not output", () => {
  assert.deepEqual(ownRunOutput("[exit 0]\n\n> tally@0.4.0 typecheck\n"), { exit: 0, output: "\n> tally@0.4.0 typecheck\n", notes: [] });
  assert.deepEqual(ownRunOutput("[exit 2]\r\nerror TS2304\n"), { exit: 2, output: "error TS2304\n", notes: [] });
  assert.deepEqual(ownRunOutput("[exit 1]\n"), { exit: 1, output: "", notes: [] });
  // Anything else stays as it was.
  assert.deepEqual(ownRunOutput("[killed by signal 9]\npartial\n"), { exit: null, output: "[killed by signal 9]\npartial\n", notes: [] });
  assert.deepEqual(ownRunOutput("ok [exit 0]\n"), { exit: null, output: "ok [exit 0]\n", notes: [] });
  assert.deepEqual(ownRunOutput(null), { exit: null, output: null, notes: [] });
});

test("a long output's digest header is its exit code, not output", () => {
  const passed = "exit 0 [full output: read_artifact out-1a2b3c4d, 183 lines, 6120 bytes]\n[the last 2 lines, 181 before them left out]\n Test Files  12 passed (12)\n      Tests  213 passed (213)";
  assert.deepEqual(ownRunOutput(passed), {
    exit: 0,
    output: "[the last 2 lines, 181 before them left out]\n Test Files  12 passed (12)\n      Tests  213 passed (213)",
    notes: [],
  });
  assert.equal(ownRunOutput("exit 1 [full output: read_artifact out-1, 900 lines, 40000 bytes]\nFAIL a.test.ts\n").exit, 1);
});

test("what run_check says after the output reads as notes in plain words", () => {
  const committed = 'Committed 1a2b3c4 "Fix rounding" with your trailer: 2 files changed, 3 insertions(+) (src/a.ts, src/a.test.ts). Nothing is left uncommitted.';
  assert.deepEqual(ownRunOutput(`[exit 0]\nok\n${committed}`), {
    exit: 0,
    output: "ok\n",
    notes: ['Committed 1a2b3c4 "Fix rounding": 2 files changed, 3 insertions(+) (src/a.ts, src/a.test.ts). Nothing is left uncommitted.'],
  });
  assert.deepEqual(ownRunOutput("[exit 1]\nFAIL\nNot committed: the check didn't pass."), {
    exit: 1,
    output: "FAIL\n",
    notes: ["Not committed: the check didn't pass."],
  });
  assert.deepEqual(ownRunOutput("[exit 0]\nNothing to commit: no change is left uncommitted."), {
    exit: 0,
    output: "",
    notes: ["Nothing to commit: no change is left uncommitted."],
  });
  const review = "[review running] Codex reviews your change for this request (1a2b..3c4d) in the background: it is over the review line, or you marked it risky. Don't give your final answer yet.";
  assert.deepEqual(ownRunOutput(`exit 0 [full output: read_artifact out-1, 90 lines, 4000 bytes]\n[the last 1 lines, 89 before them left out]\npassed\n${committed}\n\n${review}`), {
    exit: 0,
    output: "[the last 1 lines, 89 before them left out]\npassed\n",
    notes: ['Committed 1a2b3c4 "Fix rounding": 2 files changed, 3 insertions(+) (src/a.ts, src/a.test.ts). Nothing is left uncommitted.', "Codex is reviewing this change."],
  });
  // A reviewer named in two words, and a review followed by the refusal on the user's own checkout.
  assert.deepEqual(ownRunOutput(`[exit 0]\nok\n\n[review running] Claude Code reviews your change for this request.\nNot committed: this is the user's own checkout, where you commit only when they ask, with git yourself.`), {
    exit: 0,
    output: "ok\n",
    notes: ["Claude Code is reviewing this change.", "Not committed: this is your own checkout, where commits wait for you to ask."],
  });
  // A review alone after a long output's whole text, as the daemon gives it back.
  assert.deepEqual(ownRunOutput("exit 0 [full output: read_artifact out-1, 3 lines, 30 bytes]\none\npassed\n\n[review running] Codex reviews your change in the background."), {
    exit: 0,
    output: "one\npassed\n",
    notes: ["Codex is reviewing this change."],
  });
  // A line of the command's own that looks like one stays output.
  assert.deepEqual(ownRunOutput("[exit 0]\nCommitted to memory\n"), { exit: 0, output: "Committed to memory\n", notes: [] });
});

test("a CLI Bash call's failed reply starts with its exit code, which is read, not shown", () => {
  assert.deepEqual(cliRunOutput("Exit code 1\n.gitignore\nREADME.md"), { exit: 1, output: ".gitignore\nREADME.md" });
  assert.deepEqual(cliRunOutput("Exit code 127"), { exit: 127, output: "" });
  assert.deepEqual(cliRunOutput("ok\nExit code 1"), { exit: null, output: "ok\nExit code 1" });
  assert.deepEqual(cliRunOutput(null), { exit: null, output: null });
});
