import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

test("the session tab's menu has New session for a session and Remove worktree… only with a worktree, whose confirm closes it", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "session-menu.html", "session-menu-result", 60000)) as {
    error?: string;
    step?: string;
    menus: Record<string, string[]>;
    dialog: string;
    calls: unknown[];
  };
  assert.equal(seen.error, undefined, `${seen.error} (${seen.step})`);
  const base = ["Rename⌥⌘R", "Pin⌥⌘P", "Archive⇧⌘A"];
  assert.deepEqual(seen.menus.worktree, [...base, "New session", "Remove worktree…", "Copy", "Open in"]);
  assert.deepEqual(seen.menus.repo, [...base, "New session", "Copy", "Open in"]);
  assert.deepEqual(seen.menus.chat, [...base, "Copy"]);
  assert.equal(seen.dialog, "Remove worktree?");
  assert.deepEqual(seen.calls, [{ method: "closeWorkspace", conversationId: "worktree" }]);
});
