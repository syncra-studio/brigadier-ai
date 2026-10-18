import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

// The real dialog in Chromium over a faked daemon: the repositories on this computer most
// recent first, a project already added that can't be picked, search, and the three ways to
// add one (a repository from the list, a new repository).
test("Add project lists the repositories on this computer and adds or starts one", { timeout: 60000 }, async (t) => {
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
  // A new repository goes to ~/code/<its name>, git started in it.
  assert.equal(seen.newTitle, "New repository");
  assert.equal(seen.newSaveTo, "~/code/notes");
  assert.equal(seen.closedAfterNew, true);
  assert.deepEqual(seen.asked, [
    "add /Users/you/code/infra-scripts",
    "add ~/code/notes init",
  ]);
});
