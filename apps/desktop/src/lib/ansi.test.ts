import assert from "node:assert/strict";
import { test } from "node:test";

import { ansiRuns, stripAnsi } from "@/lib/ansi";

test("a command's colours become styled runs, and other escapes leave no trace", () => {
  assert.deepEqual(ansiRuns("plain"), [{ text: "plain", style: {} }]);
  assert.deepEqual(ansiRuns("\u001b[32m✓ 3 passed\u001b[0m, \u001b[1;31m1 failed\u001b[22m!\u001b[39m"), [
    { text: "✓ 3 passed", style: { color: "var(--ansi-green)" } },
    { text: ", ", style: {} },
    { text: "1 failed", style: { bold: true, color: "var(--ansi-red)" } },
    { text: "!", style: { color: "var(--ansi-red)" } },
  ]);
  // 256 colours within the palette are its tokens; others, and true colours, keep the colour before.
  assert.deepEqual(ansiRuns("\u001b[90mdim\u001b[38;5;9mred\u001b[38;5;196m\u001b[38;2;1;2;3mstill"), [
    { text: "dim", style: { color: "var(--ansi-bright-black)" } },
    { text: "red", style: { color: "var(--ansi-bright-red)" } },
    { text: "still", style: { color: "var(--ansi-bright-red)" } },
  ]);
  // Cursor moves, a window title and a link's wrapper are not text.
  assert.equal(stripAnsi("\u001b[2K\u001b[1Gdone\u001b]0;title\u0007 \u001b]8;;https://x.dev\u001b\\link\u001b]8;;\u001b\\"), "done link");
});
