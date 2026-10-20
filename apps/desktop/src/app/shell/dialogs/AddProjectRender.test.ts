import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive, renderFixturePage } from "@/fixtures/headless";

// The real dialog in Chromium over a faked daemon: the repositories on this computer most
// recent first, a project already added that can't be picked, search, and the three ways to
// add one (a repository from the list, a clone, a new repository).
test("Add project lists the repositories on this computer and adds, clones or starts one", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "add-project.html?drive=1", "add-project-result", 12000)) as Record<
    string,
    unknown
  >;
  assert.equal(seen.title, "Add project");
  assert.equal(seen.width, 760);
  assert.deepEqual(seen.list, [
    "brigadier · 2h ago · picked",
    "website · Yesterday",
    "infra-scripts · 3 days ago",
    "hello · Added · can't pick",
    "design-tokens · 2 weeks ago",
  ]);
  assert.deepEqual(seen.buttons, ["Close", "Choose folder…", "Clone from URL", "New repository", "Cancel", "Add brigadier"]);
  // Searching narrows the list and picks what's left; a click picks another.
  assert.deepEqual(seen.searched, ["website · Yesterday · picked"]);
  assert.equal(seen.addLabel, "Add website");
  assert.equal(seen.picked, "infra-scripts · 3 days ago · picked");
  assert.equal(seen.closedAfterAdd, true);
  // A clone goes to ~/code/<the repository's name>; a new repository too, git started in it.
  assert.equal(seen.cloneTitle, "Clone from URL");
  assert.equal(seen.saveTo, "~/code/website");
  assert.equal(seen.closedAfterClone, true);
  assert.equal(seen.newTitle, "New repository");
  assert.equal(seen.newSaveTo, "~/code/notes");
  assert.equal(seen.closedAfterNew, true);
  assert.deepEqual(seen.asked, [
    "add /Users/you/code/infra-scripts",
    "clone https://github.com/acme/website.git into ~/code/website",
    "add ~/code/notes init",
  ]);
});

type Frame = { ms: number; top: number; height: number; title: string | null };

/** The dialog's moves over its recorded frames: where its top edge went, and its heights in turn. */
function moves(frames: Frame[]) {
  const heights = frames.map((frame) => frame.height).filter((height, at, all) => at === 0 || height !== all[at - 1]);
  return { tops: [...new Set(frames.map((frame) => frame.top))], heights, frames: frames.length };
}

// Recorded frame by frame in real time, the scan answering late as it does (#384): the dialog
// opens where it stays, and Clone from URL and the back arrow change only its content, its top
// edge where it was and its height in one step.
for (const reducedMotion of [false, true]) {
  test(`Add project stays put as it opens, on Clone from URL and on back${reducedMotion ? " (reduced motion)" : ""}`, { timeout: 90000 }, async (t) => {
    const seen = JSON.parse(await renderFixtureLive(t, "add-project.html?frames=1", "add-project-frames", 60000, { reducedMotion })) as Record<
      "opened" | "cloned" | "back",
      Frame[]
    >;
    assert.equal(seen.opened.at(-1)?.title, "Add project");
    assert.equal(seen.cloned.at(-1)?.title, "Clone from URL");
    assert.equal(seen.back.at(-1)?.title, "Add project");
    const top = seen.opened[0]!.top;
    for (const [moment, frames] of Object.entries(seen)) {
      const moved = moves(frames);
      assert.ok(moved.frames > 20, `${moment}: frames came (${moved.frames})`);
      assert.deepEqual(moved.tops, [top], `${moment}: the top edge stays put: ${JSON.stringify(moved)}`);
      assert.ok(moved.heights.length <= 2, `${moment}: the height changes once at most: ${JSON.stringify(moved)}`);
    }
    // The list is there as it opens: no height change at all.
    assert.equal(moves(seen.opened).heights.length, 1, JSON.stringify(moves(seen.opened)));
  });
}

// Drawn with the app's shared parts (#385), in each view: the app's dialog with its title, line,
// close and back buttons, its fields, buttons, divider and badge, the picked row the app's chosen
// row and a row under the pointer the app's row wash.
test("Add project is drawn with the app's dialog, fields, buttons and rows", { timeout: 90000 }, async (t) => {
  const pairs = JSON.parse(await renderFixtureLive(t, "add-project.html?look=1", "add-project-look", 60000)) as Record<
    string,
    [unknown, unknown]
  >;
  for (const [part, [mine, shared]] of Object.entries(pairs)) {
    assert.notEqual(shared, null, `${part}: the shared part is drawn`);
    assert.deepEqual(mine, shared, `${part} looks like the app's own`);
  }
});
