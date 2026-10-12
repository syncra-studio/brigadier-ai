import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixturePage } from "@/fixtures/headless";

test("toolbar order, right sidebar, its keys, Files search, draft project folder and worker/plan-only right panel routes", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "toolbar.html", "toolbar-result", 9500)) as Record<string, unknown>;
  assert.deepEqual(seen.sessionControls, ["Context", "Terminal", "Right sidebar"]);
  assert.deepEqual(seen.draftControls, ["Context", "Terminal", "Right sidebar"]);
  assert.deepEqual(seen.views, ["Files", "Source"]);
  assert.deepEqual(seen.chatControls, ["Context", "Terminal"]);
  assert.equal(seen.files, "files");
  assert.equal(seen.closedFiles, null);
  assert.equal(seen.source, "source");
  assert.equal(seen.switchedFiles, "files");
  assert.equal(seen.atEdge, true, "the right sidebar runs the window's full height at its right edge");
  assert.equal(seen.keyClosed, null, "⌥⌘B hides the right sidebar");
  assert.deepEqual(seen.menuKey, { code: "KeyB", alt: true });
  assert.equal(seen.reopened, "files", "⌘P shows the closed right sidebar on Files");
  assert.equal(seen.workersNarrow, null, "where both don't fit, a worker click closes the right sidebar");
  assert.deepEqual(seen.sidebarWins, ["files", "collapsed"], "where both don't fit, ⌥⌘B closes the Workers panel");
  assert.equal(seen.workersInside, true, "the Workers panel sits between the content and the right sidebar");
  assert.equal(seen.searchFocused, "Search files");
  assert.equal(seen.keyboardReturns, 1, "⌘P returns the native browser keyboard to the app before focusing Files");
  assert.equal(seen.findFileFocused, "Search files");
  for (const field of ["initialRight", "filesRight", "removedKeyRight", "closedRight", "previewRight"]) assert.equal(seen[field], "collapsed", field);
  for (const field of ["workerRight", "composerRight", "contextWorkerRight"]) assert.equal(seen[field], "expanded", field);
  assert.equal(seen.workerTab, "workers");
  for (const field of ["dockClosedSlide", "dockOpenSlide"]) {
    assert.deepEqual(seen[field], { transitionProperty: "width", transitionDuration: "0.3s", sidebarEase: true, reducedMotion: true },
      `${field}: the right sidebar stays in the page and its width slides on the left sidebar's spring, and not with reduced motion`);
  }
  assert.equal(seen.dockOpeningSlides, 1, "opening the right sidebar slides its width");
  assert.equal(seen.switchSlides, 0, "switching Source to Files while open doesn't slide again");
  assert.equal(seen.closingContents, true, "the right sidebar keeps its contents through the closing slide");
  assert.equal(seen.closedContents, false, "and drops them once closed, so a closed Source reads nothing");
  for (const field of ["closedSlide", "openSlide"]) {
    assert.deepEqual(seen[field], { transitionProperty: "width", transitionDuration: "0.3s", sidebarEase: true, reducedMotion: true },
      `${field}: the right panel's width slides on the left sidebar's spring, and not with reduced motion`);
  }
  assert.ok(Number.parseFloat(String(seen.openWidth)) > 0);
  assert.equal(seen.openingShown, seen.openWidth, "the thread makes room for the panel's full width as it starts opening, not frame by frame");
  assert.equal(seen.openShown, seen.openWidth);
  assert.equal(seen.closingShown, "", "closing gives the room back at once");
  assert.equal(seen.closedShown, "");
  assert.equal(seen.planTab, "plan");
  assert.equal(seen.previewPopover, true);
  assert.equal(seen.previewReopened, false, "a chip cleared while open comes back closed");
  assert.equal(seen.draftFiles, "files");
  assert.match(String(seen.draftTree), /src.*package\.json/);
  assert.equal(seen.draftComposer, "@package.json ");
  assert.deepEqual(seen.draftMention, [{ type: "file", path: "package.json" }]);
  assert.ok((seen.draftSource as string[]).includes("Stage all"), "a local draft stages in the project folder");
  assert.equal(seen.draftCommitBox, true);
  assert.equal(seen.draftNote, false);
  assert.equal(seen.closedSource, null);
  assert.equal(seen.keptMessage, "Unsent message", "closing Source keeps the unsent commit message");
  assert.match(String(seen.worktreeNote), /own copy/);
  assert.ok(!(seen.worktreeSource as string[]).some((label) => /Stage|Discard/.test(label ?? "")), "a worktree draft only reads");
  assert.equal(seen.worktreeCommitBox, false);
  assert.equal(seen.chatPane, null);
  assert.match(String(seen.chatText), /Nothing here yet/);
  assert.doesNotMatch(String(seen.chatText), /Plan|Workers/);
  assert.equal(seen.contextClosed, "false");
  assert.equal(seen.contextOpened, "true");
  assert.match(String(seen.chatUsage), /1,200 tokens used/);
  assert.doesNotMatch(String(seen.chatUsage), /Plan|Workers/);
  assert.equal((seen.savedPane as { state: { open: boolean } }).state.open, true);
});
