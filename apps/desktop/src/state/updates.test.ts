import assert from "node:assert/strict";
import { test } from "node:test";

import type { UpdateItem, UpdatesView } from "@/ipc/generated";

import { finishedKeys, pillClick, shownUpdates } from "./updates";

const run: UpdateItem["action"] = { type: "run", command: "npm i -g @openai/codex" };

function item(target: UpdateItem["target"], progress: UpdateItem["progress"]["type"], action: UpdateItem["action"] = run): UpdateItem {
  return {
    target,
    current: "1.0.0",
    latest: "1.1.0",
    action,
    progress: progress === "failed" ? { type: "failed", error: "no network" } : { type: progress },
  };
}

test("a lone agent update runs from the pill; anything else opens the list", () => {
  const codex = item("codex", "available");
  assert.equal(pillClick([codex]), codex);
  // Several, Brigadier's own download, a command to copy, one running or failed: the list.
  assert.equal(pillClick([codex, item("claude", "available")]), "menu");
  assert.equal(pillClick([item("app", "available", { type: "download", url: "https://example.com" })]), "menu");
  assert.equal(pillClick([item("codex", "available", { type: "manual", command: "npm i -g codex" })]), "menu");
  assert.equal(pillClick([item("codex", "updating")]), "menu");
  assert.equal(pillClick([item("codex", "failed")]), "menu");
  assert.equal(pillClick([]), "menu");
});

test("finished updates are due to leave until seen, then no longer show", () => {
  const view: UpdatesView = {
    items: [item("codex", "updated"), item("claude", "updating")],
    checkedAtMs: null,
  };
  assert.deepEqual(finishedKeys(view, []), ["codex@1.1.0"]);
  assert.deepEqual(finishedKeys(view, ["codex@1.1.0"]), []);
  assert.deepEqual(
    shownUpdates(view, [], ["codex@1.1.0"]).map((entry) => entry.target),
    ["claude"],
  );
  assert.deepEqual(finishedKeys(null, []), []);
});
