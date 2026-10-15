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
  };
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
});
