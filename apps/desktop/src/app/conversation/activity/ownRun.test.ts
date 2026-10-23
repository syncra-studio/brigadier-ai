import assert from "node:assert/strict";
import { test } from "node:test";

import { cliRunOutput } from "@/app/conversation/activity/ownRun";

test("a CLI Bash call's failed reply starts with its exit code, which is read, not shown", () => {
  assert.deepEqual(cliRunOutput("Exit code 1\n.gitignore\nREADME.md"), { exit: 1, output: ".gitignore\nREADME.md" });
  assert.deepEqual(cliRunOutput("Exit code 127"), { exit: 127, output: "" });
  assert.deepEqual(cliRunOutput("ok\nExit code 1"), { exit: null, output: "ok\nExit code 1" });
  assert.deepEqual(cliRunOutput(null), { exit: null, output: null });
});
