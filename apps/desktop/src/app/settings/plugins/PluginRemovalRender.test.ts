import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixturePage } from "@/fixtures/headless";
import type { Selection } from "@/state/store";

type Snapshot = {
  selection: Selection;
  history: { entries: Selection[]; index: number };
  title: string;
  dialog: string;
};
type Outcome = {
  pending: { cancel: boolean; close: boolean };
  before: Snapshot;
  after: Snapshot;
  refreshed: boolean;
  removed: boolean;
};

test("deferred plugin removal refreshes after dismissal or navigation without redirecting, and replaces active detail on success", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "plugins.html?drive=removal", "plugins-result", 20000)) as Record<string, Outcome> & { error?: string };
  assert.equal(seen.error, undefined);
  const list: Selection = { type: "settings", page: "plugins" };
  const detail: Selection = { type: "settings", page: "plugins", detail: { plugin: "server:linear" } };
  const general: Selection = { type: "settings", page: "general" };
  for (const scenario of ["cancel", "close-general", "close-back", "navigate-back", "active"]) {
    const result = seen[scenario]!;
    assert.deepEqual(result.pending, { cancel: false, close: false }, `${scenario}: dismissal stays enabled`);
    assert.equal(result.refreshed, true, `${scenario}: completion still refreshes`);
    assert.equal(result.removed, true, `${scenario}: refreshed data reflects the removal`);
    assert.equal(result.after.dialog, "", `${scenario}: no stale dialog`);
    if (scenario !== "active") {
      assert.deepEqual(result.after.selection, result.before.selection, `${scenario}: no completion redirect`);
      assert.deepEqual(result.after.history, result.before.history, `${scenario}: Back/Forward untouched`);
    }
  }
  assert.deepEqual(seen.cancel!.after.selection, detail);
  assert.deepEqual(seen["close-general"]!.after.selection, general);
  for (const scenario of ["close-back", "navigate-back"]) {
    assert.deepEqual(seen[scenario]!.after.history, { entries: [list, detail, general], index: 1 });
  }
  const active = seen.active!;
  assert.deepEqual(active.after.selection, list);
  assert.equal(active.after.title, "Plugins");
  assert.deepEqual(active.after.history, { entries: [list, list, general], index: 1 });
});
