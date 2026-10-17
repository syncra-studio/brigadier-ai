import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("the thread's own work reads like the reference thread: running commands stream, a row per edited file, image rows, background terminals, the turn's changes from its own record", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "thread-work.html", "thread-work-result", 90000)) as Record<string, unknown> & { error?: string };
  assert.equal(seen.error, undefined);
  const live = seen as unknown as {
    liveChildren: string[];
    liveFirst: { outputs: string[]; commands: string[]; endings: number };
    liveLater: { outputs: string[]; readAgain: boolean };
  };
  // An open live group: the read is done, the running commands tick ("Running command for 3s").
  assert.equal(live.liveChildren[0], "Read math.js");
  for (const row of live.liveChildren.slice(1)) assert.match(row, /^Running command for \ds$/);
  // Each opens to its Shell box while it runs: its command, its output so far, no ending yet.
  assert.deepEqual(live.liveFirst.commands, ["$ npm run build", "$ cargo test"]);
  assert.deepEqual(live.liveFirst.outputs, ["compiling 1/3", "test a ... ok"]);
  assert.equal(live.liveFirst.endings, 0);
  // More output: the boxes follow it, read again.
  assert.equal(live.liveLater.readAgain, true);
  assert.deepEqual(live.liveLater.outputs, ["compiling 1/3 compiling 2/3 compiling 3/3", "test a ... ok test b ... ok"]);

  // A finished command's whole output past 8 KiB, not an excerpt.
  const big = seen.big as { bytes: number; last: string; ending: string; clipped: boolean };
  assert.ok(big.bytes > 8 * 1024, `${big.bytes} bytes shown`);
  assert.match(big.last, /^line 400 /);
  assert.deepEqual([big.ending, big.clipped], ["Success", false]);

  // A row per file: verb, link, counts, the dot of a file made or deleted.
  assert.deepEqual(seen.edit, [
    { row: "Edited math.js +7 −2", link: "math.js", counts: "+7 −2", dot: null },
    { row: "Created new.ts +3 −0", link: "new.ts", counts: "+3 −0", dot: "add" },
    { row: "Deleted old.ts +0 −4", link: "old.ts", counts: "+0 −4", dot: "delete" },
  ]);
  // A shell command's edit, which its request owns, reads as an edit after the command's row (#87).
  assert.deepEqual(seen.shellEdit, ["Edited notes.md +1 −0"]);
  // The name opens the file, not the row; the row opens to the file's numbered diff.
  assert.deepEqual(seen.editLink, { opened: ["/repo/src/math.js"], rowOpened: false });
  const diff = seen.editDiff as { row: string; name: string; numbers: string[] };
  assert.equal(diff.name, "math.js");
  // Numbered from the real patch's hunk (`@@ -10,6 +10,11 @@`), not from 1.
  assert.deepEqual(diff.numbers, ["10", "11", "12"]);
  // Opened, the row keeps the file's name and leaves the counts to the diff card (#175).
  assert.equal(diff.row, "Edited math.js");

  // A Codex find in a page says what and where.
  assert.equal(seen.find, "Searched the web for 'LTS' in https://nodejs.org/en");

  // Images: their own rows, opening to tiles; the turn's made images gathered.
  assert.deepEqual(seen.images, { viewed: "Viewed 2 images", made: "Created an image" });
  assert.deepEqual(seen.viewedTiles, ["Inspected image", "Inspected image"]);
  assert.deepEqual(seen.gathered, ["Created image"]);

  // Background terminals, by how they stand; a finished one opens to its output.
  assert.deepEqual(seen.background, [
    { row: "Started background terminal with npm run dev -- --port 5173", state: "running", icon: "run" },
    // A finished one names its command too.
    { row: "Background terminal finished with npm run dev -- --port 5173", state: "finished", icon: "run" },
    { row: "Background terminal stopped with npm run dev -- --port 5173", state: "stopped", icon: "stop" },
  ]);
  assert.deepEqual(seen.backgroundOutput, { output: "served on :5173 shut down", ending: "Success", asked: ["run-2"] });

  // The turn's changes from the request's own record: one file named; four listed, three then more.
  const turnDiff = seen.turnDiff as Record<"one" | "four", { title: string; counts: string; files: number; more: string | null; undo: string | null; held?: string | null }>;
  assert.deepEqual(
    [turnDiff.one.title, turnDiff.one.counts, turnDiff.one.files, turnDiff.one.undo],
    ["Edited run.mjs", "+4 −1", 0, "Undo"],
  );
  assert.deepEqual(
    [turnDiff.four.title, turnDiff.four.counts, turnDiff.four.files, turnDiff.four.more, turnDiff.four.undo],
    ["Edited 4 files", "+11 −2", 3, "Show 1 more file", "Undo"],
  );
  // Undo says beforehand, and afterwards, which file it leaves as it is.
  assert.equal(turnDiff.four.held, "You changed src/notes.md during this request too, so Undo leaves it as it is.");
  assert.deepEqual(seen.undone, {
    asked: ["four:undo"],
    toast: "Changes reverted. You changed src/notes.md during this request too, so Undo leaves it as it is.",
  });
  assert.equal(seen.reapply, "Reapply");
  // Reapply names what Undo left without giving Undo's reason again; a refusal says which way.
  assert.equal(seen.reapplied, "Changes reapplied. notes.md stays as it is: Undo left it");
  assert.equal(seen.refused, "Couldn't undo the changes: Later changes touch the same files (run.mjs)");

  // The working request's pill, from its record; a click opens its first file's tab.
  assert.match(String(seen.pill), /^2 files changed ?\+11 −2$/);
  assert.equal(seen.pillTab, "src/math.js");
});
