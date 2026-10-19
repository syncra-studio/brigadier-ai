import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

const AUTOMATIC = "A fast, light model Brigadier picks for each message. When an agent is out of quota, another one writes.";
const WRITER = "The model that writes a commit message when you commit from the Source panel without one";

// The real page in Chromium over a faked daemon: the writer model and its Automatic choice, the
// styles, an instructions box that saves on ⌘S, once typing pauses and as it goes, and the
// branch prefix checked by git's rules.
test("git settings: writer model, styles, instructions that save themselves, branch prefix", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "git.html?drive=1", "git-result", 20000)) as Record<string, unknown>;
  assert.deepEqual(seen.sections, ["Writing", "Merging", "Branches"]);
  assert.deepEqual(seen.rows, [
    "Write with",
    "Commit messages",
    "Leave out AI co-author lines",
    "Pull requests",
    "Merge on its own when the work is ready",
    "Branch prefix",
  ]);
  // Merging on its own: off by default, one switch, and a click turns it on and saves it.
  assert.deepEqual(seen.autoMerge, {
    description:
      "The agent checks its work, opens a pull request when the project is on GitHub, and merges it. Off, it tells you the work is ready and you merge",
    checked: "false",
  });
  assert.deepEqual(seen.autoMergeOn, { checked: "true", saved: true });
  assert.equal(seen.autoMergeWritten, true);
  assert.equal(seen.writer, "Automatic");
  // Automatic's own line is in the menu (below), not under the row.
  assert.equal(seen.writerLine, WRITER);
  assert.equal(seen.boxes, 0, "no instruction box until Custom instructions");
  assert.equal(seen.picked, "Sonnet 5.5Low");
  assert.equal(seen.gone, "Not available now, so Automatic writes them");
  assert.deepEqual((seen.writerMenu as string[])[0], `Automatic${AUTOMATIC}`);
  assert.deepEqual(seen.automatic, { trigger: "Automatic", saved: null });
  assert.deepEqual(seen.styles, [
    "Follow this repositoryWritten like the repository’s recent commits.",
    "Conventional CommitsA type and a short summary, like “fix: keep the session after a restart”.",
    "Custom instructionsYour own guidance, written here.",
  ]);
  assert.equal(seen.commitStyle, "custom");
  assert.equal(seen.boxShown, true);
  assert.equal(seen.placeholder, "Add commit message guidance…");
  // Save while there's an edit; ⌘S saves it, and "Saved" shows.
  assert.equal(seen.dirty, "Save");
  assert.deepEqual(seen.afterCmdS, { end: "Saved", saved: "Start with [Area]." });
  // Typing on saves by itself 3 s after it pauses, not before; "Saved" then goes.
  assert.equal(seen.beforePause, "Start with [Area].");
  assert.deepEqual(seen.afterPause, { end: "Saved", saved: "Start with [Area]. Present tense." });
  assert.equal(seen.quiet, "");
  // A prefix git refuses says why and isn't saved; one it takes is, on Enter.
  assert.equal(seen.refused, 'The prefix can\'t contain "..".');
  assert.equal(seen.prefixKept, "brigadier/");
  assert.equal(seen.prefixSaved, "sg/");
  // Another style picked while the box had an edit: the edit was saved as the box went.
  assert.deepEqual(seen.pr, { style: "conventional", instructions: "Title under 70 characters.", box: false });
  // Typed on while a save was on its way, then left: the newer edit is saved after it.
  assert.equal(seen.prDuringSave, "Title under 70 characters. Body in bullets. No emoji.");
});
