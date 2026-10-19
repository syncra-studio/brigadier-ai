import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

for (const session of [false, true]) {
  test(`in a ${session ? "session" : "Chat"}, stored outputs open without a missing-file notice and dead absolute links still offer their saved copy`, { timeout: 90000 }, async (t) => {
    const seen = JSON.parse(await renderFixtureLive(t, `file-chip.html${session ? "?session" : ""}`, "file-chip-result", 60000)) as {
      error?: string;
      before: Record<string, string | null>;
      after: Record<string, string | null>;
      saves: Record<string, boolean>;
      toasts: number;
      cellIconOnNameLine: boolean;
      calls: Record<string, unknown>[];
    };
    assert.equal(seen.error, undefined, `${seen.error} ${JSON.stringify(seen.calls)}`);
    assert.deepEqual(seen.before, { stored: null, lost: null, outputs: null, dotOutputs: null });
    assert.match(seen.after.stored!, /This file is no longer here; Brigadier kept a copy: Save…/);
    assert.match(seen.after.lost!, /This file is no longer here\)/);
    assert.equal(seen.after.outputs, null, "an outputs/ link must not say the file is no longer here");
    assert.equal(seen.after.dotOutputs, null, "a ./outputs/ link must not say the file is no longer here");
    assert.deepEqual(seen.saves, { stored: true, lost: false, outputs: false, dotOutputs: false });
    assert.equal(seen.toasts, 0);
    // In a narrow table cell the chip's icon stays on its name's line.
    assert.equal(seen.cellIconOnNameLine, true);
    const commands = seen.calls.map((call) => call.command ?? call.method);
    assert.deepEqual(commands, ["resolveFileLink", "open_artifact", "resolveFileLink", "resolveFileLink", "open_artifact", "resolveFileLink", "open_artifact", "save_artifact"]);
    assert.deepEqual(seen.calls[0], {
      method: "resolveFileLink",
      conversationId: "chat-1",
      link: "/var/folders/xy/T/brigadier/scratch/task-1/outputs/rounding.md:3",
      path: "/var/folders/xy/T/brigadier/scratch/task-1/outputs/rounding.md",
      atMs: 1700,
    });
    // A chip outside a message passes no time.
    assert.equal(seen.calls[2]!.atMs, undefined);
    assert.equal(seen.calls[1]!.id, "blob-1");
    for (const index of [4, 6, 7]) {
      assert.equal(seen.calls[index]!.id, "blob-1");
      assert.equal(seen.calls[index]!.fileName, "rounding.md");
    }
    assert.equal(seen.calls[3]!.link, "outputs/task-1/rounding.md");
    assert.equal(seen.calls[3]!.path, session ? "/repo/outputs/task-1/rounding.md" : "outputs/task-1/rounding.md");
    assert.equal(seen.calls[5]!.link, "./outputs/task-1/rounding.md");
    assert.equal(seen.calls[5]!.path, session ? "/repo/outputs/task-1/rounding.md" : "./outputs/task-1/rounding.md");
  });
}
