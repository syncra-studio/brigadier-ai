import assert from "node:assert/strict";
import { test } from "node:test";

import { ownRunOutput } from "@/app/conversation/activity/ownRun";

test("an own command's [exit N] line is its exit code, not output", () => {
  assert.deepEqual(ownRunOutput("[exit 0]\n\n> tally@0.4.0 typecheck\n"), { exit: 0, output: "\n> tally@0.4.0 typecheck\n" });
  assert.deepEqual(ownRunOutput("[exit 2]\r\nerror TS2304\n"), { exit: 2, output: "error TS2304\n" });
  assert.deepEqual(ownRunOutput("[exit 1]\n"), { exit: 1, output: "" });
  // Anything else stays as it was.
  assert.deepEqual(ownRunOutput("[killed by signal 9]\npartial\n"), { exit: null, output: "[killed by signal 9]\npartial\n" });
  assert.deepEqual(ownRunOutput("ok [exit 0]\n"), { exit: null, output: "ok [exit 0]\n" });
  assert.deepEqual(ownRunOutput(null), { exit: null, output: null });
});
