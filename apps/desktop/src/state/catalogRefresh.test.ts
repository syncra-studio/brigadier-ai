import assert from "node:assert/strict";
import { test } from "node:test";

import type { Conversation } from "@/ipc/generated";
import { trackCatalogChanges } from "@/state/catalogRefresh";
import { useApp } from "@/state/store";

const row = (extra: Record<string, unknown>) => ({ id: "c", title: "C", updatedAtMs: 0, ...extra }) as unknown as Conversation;

test("a field removed live (Undo of Done) stays removed when an older snapshot lands", () => {
  useApp.setState({ conversations: { c: row({ doneAtMs: 5, snoozedUntilMs: 9 }) } });
  const changes = trackCatalogChanges();
  useApp.setState({ conversations: { c: row({ snoozedUntilMs: 9 }) } });
  const merged = changes.merge({ projects: [], conversations: [row({ doneAtMs: 5, snoozedUntilMs: 9, title: "Renamed" })] });
  changes.stop();
  const [c] = merged.conversations;
  assert.equal("doneAtMs" in (c ?? {}), false);
  assert.equal(c?.snoozedUntilMs, 9);
  assert.equal(c?.title, "Renamed");
});
