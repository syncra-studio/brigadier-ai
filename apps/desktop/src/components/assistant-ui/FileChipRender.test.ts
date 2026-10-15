import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

test("a dead file link says the file is no longer here, opens and saves the stored copy of a worker's file, and never toasts an error", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "file-chip.html", "file-chip-result", 60000)) as {
    error?: string;
    before: Record<string, string | null>;
    after: Record<string, string | null>;
    saves: Record<string, boolean>;
    toasts: number;
    calls: Record<string, unknown>[];
  };
  assert.equal(seen.error, undefined, `${seen.error} ${JSON.stringify(seen.calls)}`);
  assert.deepEqual(seen.before, { stored: null, lost: null });
  assert.match(seen.after.stored!, /This file is no longer here; Brigadier kept a copy: Save…/);
  assert.match(seen.after.lost!, /This file is no longer here\)/);
  assert.deepEqual(seen.saves, { stored: true, lost: false });
  assert.equal(seen.toasts, 0);
  const commands = seen.calls.map((call) => call.command ?? call.method);
  assert.deepEqual(commands, ["resolveFileLink", "open_artifact", "resolveFileLink", "save_artifact"]);
  assert.deepEqual(seen.calls[0], {
    method: "resolveFileLink",
    conversationId: "chat-1",
    link: "/var/folders/xy/T/brigadier/scratch/task-1/outputs/rounding.md:3",
    atMs: 1700,
  });
  // A chip outside a message passes no time.
  assert.equal(seen.calls[2]!.atMs, undefined);
  assert.equal(seen.calls[1]!.id, "blob-1");
  assert.equal(seen.calls[3]!.id, "blob-1");
  assert.equal(seen.calls[3]!.fileName, "rounding.md");
});
