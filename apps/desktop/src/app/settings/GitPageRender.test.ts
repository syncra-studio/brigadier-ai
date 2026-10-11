import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

const AUTOMATIC = "A fast, light model Brigadier picks for each message. When an agent is out of quota, another one writes.";
const WRITER = "The model that writes a commit message when you press Generate in the Source panel.";

// The real page in Chromium over a faked daemon: the writer model and its Automatic choice, the
// styles, an instructions box that saves on ⌘S, once typing pauses and as it goes, and the
// branch prefix checked by git's rules.
test("git settings: writer model, styles, instructions that save themselves, branch prefix", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "git.html?drive=1", "git-result", 20000)) as Record<string, unknown>;
  assert.deepEqual(seen.sections, ["Writing", "Branches"]);
  assert.deepEqual(seen.rows, [
    "Write with",
    "Commit messages",
    "Leave out AI co-author lines",
    "Pull requests",
    "Branch prefix",
  ]);
  assert.equal(seen.writer, "Automatic");
  assert.equal(seen.writerLine, `${WRITER} Automatic: a fast, light model Brigadier picks for each message. When an agent is out of quota, another one writes.`);
  assert.equal(seen.boxes, 0, "no instruction box until Custom instructions");
  assert.equal(seen.picked, "Sonnet 5.5Low");
  assert.equal(seen.gone, `${WRITER} It isn't available now, so Automatic writes them.`);
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
});
