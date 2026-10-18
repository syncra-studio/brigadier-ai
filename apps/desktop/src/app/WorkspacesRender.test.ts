import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

// The real sidebar in Chromium over a faked daemon, workspaces on: one row per workspace with
// the strongest state of its tabs and their count, and a click opening the tab had open last.
test("sidebar: a project's workspaces are rows with the strongest state of their tabs", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "workspaces.html", "workspaces-result", 20000)) as {
    rows: { title: string; status: string | null; active: boolean }[];
    opened: unknown;
    activeAfter: string[];
  };
  assert.deepEqual(
    seen.rows.map(({ title, status }) => [title, status]),
    [
      ["Redesigning the header", "working:2 working"],
      ["Fix the login", "ready:Ready"],
      ["Tidy the docs", "merged:Merged"],
      ["Waiting on a limit", "needsYou:Needs you"],
      ["Lone worker", "running:"],
    ],
  );
  assert.deepEqual(seen.opened, { type: "conversation", id: "w1-b" }, "the row opens the tab had open last");
  assert.deepEqual(seen.activeAfter, ["Redesigning the header"]);
});
