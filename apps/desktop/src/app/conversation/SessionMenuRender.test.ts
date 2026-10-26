import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

test("the session tab's menu: Rename, Copy (path, id), Archive and Permanently delete last, no Remove worktree…; ⌥⌘P still pins", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "session-menu.html", "session-menu-result", 60000)) as {
    error?: string;
    step?: string;
    menus: Record<string, string[]>;
    copies: Record<string, string[]>;
    copied: string[];
    deleteDialog: string;
    calls: unknown[];
  };
  assert.equal(seen.error, undefined, `${seen.error} (${seen.step})`);
  const last = ["Archive⇧⌘A", "Permanently delete"];
  assert.deepEqual(seen.menus.worktree, ["Rename⌥⌘R", "Copy", "Open in", ...last]);
  assert.deepEqual(seen.menus.repo, ["Rename⌥⌘R", "Copy", "Open in", ...last]);
  assert.deepEqual(seen.menus.chat, ["Rename⌥⌘R", "Copy", ...last]);
  assert.deepEqual(seen.copies.worktree, ["Copy path", "Copy session id", "Copy as Markdown"]);
  assert.deepEqual(seen.copies.chat, ["Copy chat id", "Copy as Markdown"]);
  // The id the CLI shows (`brigadier list`): the conversation id's last 8 characters.
  assert.deepEqual(seen.copied, ["362537be", "/tmp/session-menu-fixture/session"]);
  assert.equal(seen.deleteDialog, "Delete session?");
  assert.deepEqual(seen.calls, [
    { method: "setPinned", id: "019a2b3c-4d5e-7f60-8a9b-0c1d362537be", pinned: true },
    { method: "delete", ids: ["019a2b3c-4d5e-7f60-8a9b-0c1d362537be"] },
  ]);
});
