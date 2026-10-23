import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

type Group = { label: string; rows: string[] };

const labels = (groups: unknown) => (groups as Group[]).map((group) => [group.label, group.rows]);

// The real page in Chromium over a seeded store: search, the kind/order and project menus,
// picking after filtering and sorting, the picked bar, the "…" menu and the right-click menu.
test("archived chats search, filter, sort, pick and delete", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "archived.html?drive=1", "archived-result", 12000)) as Record<
    string,
    unknown
  >;
  assert.deepEqual(labels(seen.first), [
    ["storefront-web", ["Checkout page redesign", "Fix flaky cart tests"]],
    ["billing-api", ["Add invoice export", "Billing webhooks retry"]],
    ["Chats", ["Trip to Lisbon", "Takeaway app plan"]],
  ]);
  assert.deepEqual(seen.triggers, ["All chats", "All projects"]);
  assert.deepEqual(labels(seen.searched), [["billing-api", ["Billing webhooks retry"]]]);
  assert.equal(seen.noMatch, "No archived chats match");
  assert.deepEqual(seen.kindMenu, ["All chats", "Sessions", "Chats", "Updated", "Created", "Alphabetical"]);
  assert.deepEqual(labels(seen.sessionsByName), [
    ["billing-api", ["Add invoice export", "Billing webhooks retry"]],
    ["storefront-web", ["Checkout page redesign", "Fix flaky cart tests"]],
  ]);
  assert.deepEqual(seen.projectMenu, ["All projects", "billing-api", "storefront-web", "Chats"]);
  assert.deepEqual(labels(seen.oneProject), [["storefront-web", ["Checkout page redesign", "Fix flaky cart tests"]]]);
  assert.equal(seen.trigger, "storefront-web");
  // Picking follows the filtered, sorted order: Shift-click picks from the last ⌘-clicked row.
  assert.equal(seen.pickedTwo, "2 pickedClearUnarchive (2)Delete (2)…");
  assert.equal(seen.pickedRange, "2 pickedClearUnarchive (2)Delete (2)…");
  assert.deepEqual(seen.selectedRows, ["Checkout page redesign", "Fix flaky cart tests"]);
  assert.deepEqual(seen.afterUnarchive, {
    groups: [{ label: "billing-api", rows: ["Add invoice export", "Billing webhooks retry"] }],
    bar: "",
  });
  assert.equal(
    seen.deleteDialog,
    "Delete session?This permanently deletes “Billing webhooks retry”. This can't be undone.CancelDelete sessionClose",
  );
  assert.deepEqual(labels(seen.afterDelete), [["billing-api", ["Add invoice export"]]]);
  assert.deepEqual(seen.contextMenu, ["Unarchive", "Delete…"]);
  assert.deepEqual(seen.moreMenu, ["Delete all…"]);
  assert.match(String(seen.deleteAll), /^Delete 3 conversations\?/);
  // Delete is ready at once; the unlanded-work check answers later, adding its line.
  assert.doesNotMatch(String(seen.deleteAll), /never landed/);
  assert.deepEqual(seen.deleteAtOnce, { enabled: true, focused: true });
  assert.match(String(seen.unlanded), /branch brigadier\/912a9469\/session has work that never landed/);
  // The box keeps its text while it animates out (it once emptied first).
  assert.match(String(seen.closing), /^Delete 3 conversations\?/);
  assert.deepEqual(seen.calls, ["restore s1,s2", "delete s4"]);
});

// Nothing archived yet: the empty line names what the page holds, sessions and chats, not just
// chats (#583).
test("archived chats, nothing archived yet, says what the page holds", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(
    await renderFixturePage(t, "archived.html?empty=1&drive=1", "archived-result", 12000),
  ) as Record<string, unknown>;
  assert.equal(seen.emptyState, "No archived sessions or chats");
});
