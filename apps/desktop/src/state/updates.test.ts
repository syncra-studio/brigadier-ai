import assert from "node:assert/strict";
import { test } from "node:test";

import type { UpdateItem, UpdatesView } from "@/ipc/generated";

import { applyUpdateEvents, finishedKeys, pillClick, shownUpdates, useUpdates } from "./updates";

const run: UpdateItem["action"] = { type: "run", command: "npm i -g codex" };

function item(target: UpdateItem["target"], progress: UpdateItem["progress"]["type"], action: UpdateItem["action"] = run): UpdateItem {
  return {
    target,
    current: "1.0.0",
    latest: "1.1.0",
    action,
    progress: progress === "failed" ? { type: "failed", error: "no network" } : { type: progress },
  };
}

function publish(items: UpdateItem[]): void {
  applyUpdateEvents([{
    seq: 1,
    stream: "updates",
    streamSeq: 1,
    atMs: 1,
    event: { type: "updatesChanged", updates: { items, checkedAtMs: 1 } },
  }]);
}

function shownTargets(): UpdateItem["target"][] {
  const { view, skipped, seen } = useUpdates.getState();
  return shownUpdates(view, skipped, seen).map((row) => row.target);
}

test("a lone agent update runs from the pill; anything else opens the list", () => {
  const codex = item("codex", "available");
  assert.equal(pillClick([codex]), codex);
  const claude = item("claude", "available");
  assert.equal(pillClick([claude]), claude);
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

test("a finished row leaves after one second, without hiding a running or newly available update", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const initial = useUpdates.getState();
  t.after(() => useUpdates.setState(initial));
  useUpdates.setState({ view: null, seen: [], skipped: [], errors: {} });
  publish([item("codex", "updated"), item("claude", "updating")]);
  t.mock.timers.tick(600);
  // A repeated snapshot must not restart the check's linger time.
  publish([item("codex", "updated"), item("claude", "updating")]);
  t.mock.timers.tick(399);
  assert.deepEqual(shownTargets(), ["codex", "claude"]);
  t.mock.timers.tick(1);
  assert.deepEqual(shownTargets(), ["claude"]);

  // A newer version for the same agent must not be dismissed with the old version.
  publish([{ ...item("codex", "available"), latest: "1.2.0" }, item("claude", "failed")]);
  t.mock.timers.tick(2000);
  assert.deepEqual(shownTargets(), ["codex", "claude"]);
  publish([{ ...item("codex", "updated"), latest: "1.2.0" }, item("claude", "failed")]);
  t.mock.timers.tick(1000);
  assert.deepEqual(shownTargets(), ["claude"], "failed updates stay available to retry");
});
