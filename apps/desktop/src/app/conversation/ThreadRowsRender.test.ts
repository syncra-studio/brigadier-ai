import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

type Shell = {
  command: string | null; clamped: boolean; outputLines: number; output: string | null; colored: [string, string][];
  ending: string | null; clipped: string | null; folded: boolean;
};

test("the thread's rows read like the reference thread: Shell boxes, stopped steps, a live group's children, web searches", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "thread-rows.html", "thread-rows-result", 90000)) as Record<
    "shell-long" | "shell-failed" | "shell-running" | "shell-clipped" | "shell-command",
    Shell
  > & {
    error?: string;
    commandClicked: boolean;
    stopped: { row: string | null; state: string | null; shimmer: boolean };
    stoppedShell: string | null;
    groupRow: string | null;
    children: (string | null)[];
    search: { row: string | null; opens: boolean };
    chips: [string, string][];
    code: string[];
    asked: string[];
  } & Record<"diff-patch" | "diff-edit" | "diff-empty" | "diff-missing", {
    name: string | null; counts: string | null; numbers: string[]; kinds: string[]; gaps: string[]; marks: string[]; empty: string | null; coloured: boolean;
  }>;
  assert.equal(seen.error, undefined);
  // The whole output, newest line in view, no head-and-tail fold; its colours kept; success said.
  const long = seen["shell-long"];
  assert.deepEqual([long.outputLines, long.folded, long.ending, long.clipped], [60, false, "Success", null]);
  assert.deepEqual(long.colored, [["✓ 60 tests passed", "var(--ansi-green)"]]);
  assert.ok(!(long.output ?? "").includes(String.fromCharCode(27)), "no escape left in the output");
  assert.equal(seen["shell-failed"].ending, "Exit code 1");
  // Running: nothing printed yet and no ending.
  assert.deepEqual([seen["shell-running"].output, seen["shell-running"].ending], [null, null]);
  // Cut short by the daemon: says so, and the marker isn't shown as output.
  assert.equal(seen["shell-clipped"].output, "head of the log");
  assert.equal(seen["shell-clipped"].clipped, "Only the start was kept: 12 KB more not shown");
  // A long command: two lines until clicked.
  assert.deepEqual([seen["shell-command"].clamped, seen.commandClicked], [true, false]);
  // A command the user's Stop cut: "Stopped … after 4s", no shimmer; its box ends "Stopped".
  assert.deepEqual(seen.stopped, { row: "Stopped sleep 30 after 4s", state: "stopped", shimmer: false });
  assert.equal(seen.stoppedShell, "Stopped");
  // Opened while live: the running command ticks, the read still running is left out.
  assert.equal(seen.groupRow, "Reading round.js");
  assert.equal(seen.children.length, 2);
  assert.equal(seen.children[0], "Read math.js");
  assert.match(seen.children[1] ?? "", /^Running command for [3-9]s$/);
  // A web search: its site after a bar, nothing to open.
  assert.deepEqual(seen.search, { row: "Searched the web for node lts release | nodejs.org", opens: false });
  // A real patch: numbered from its hunks, the stretch between them counted, changed words marked, coloured.
  const patch = seen["diff-patch"];
  assert.deepEqual([patch.name, patch.counts], ["math.js", "+2 −1"]);
  assert.deepEqual(patch.numbers, ["1", "2", "2", "3", "4", "20", "21", "22"]);
  assert.deepEqual(patch.kinds, ["context", "del", "add", "context", "context", "context", "add", "context"]);
  assert.deepEqual(patch.gaps, ["⋯ 15 unchanged lines"]);
  // (Marks are cut where syntax colours change.)
  assert.equal(patch.marks.join(""), "(+ Number.EPSILON)");
  assert.equal(patch.coloured, true);
  // An edit rebuilt from its call: no numbers, its changed word marked on both sides.
  assert.deepEqual([seen["diff-edit"].numbers, seen["diff-edit"].marks, seen["diff-edit"].counts], [[], ["1", "2"], "+1 −1"]);
  assert.equal(seen["diff-empty"].empty, "No changes");
  assert.equal(seen["diff-missing"].empty, "Diff too large to preview");
  // Inline code naming a checkout file is a chip: from the list, or asked by its exact path
  // because the list was cut short; other code stays code, and a missing file is no chip.
  assert.deepEqual(seen.chips, [["src/cli.ts (line 64)", "/repo/src/cli.ts:64"], ["src/made-later.ts", "/repo/src/made-later.ts"]]);
  assert.deepEqual(seen.code, ["Math.round", "src/gone.ts"]);
  assert.deepEqual(seen.asked, ["/repo/src/gone.ts", "/repo/src/made-later.ts"]);
});
