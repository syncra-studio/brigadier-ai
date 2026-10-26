import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

test("toolbar order, right sidebar, its keys, Files search, draft project folder and worker-only right panel routes", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "toolbar.html?live=1", "toolbar-result", 30000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined);
  assert.deepEqual(seen.sessionControls, ["Context", "Terminal", "Right sidebar"]);
  assert.deepEqual(seen.draftControls, ["Context", "Terminal", "Right sidebar"]);
  assert.deepEqual(seen.views, ["Files", "Search", "Source"]);
  assert.deepEqual(seen.chatControls, ["Chat actions", "Context", "Terminal"]);
  assert.equal(seen.actionsRest, seen.contextRest, "the actions button shares the toolbar's muted color");
  for (const field of ["actionsHover", "actionsFocus", "actionsOpen"]) assert.equal(seen[field], seen.contextHover, field);
  assert.notEqual(seen.actionsRest, seen.actionsHover);
  assert.equal(seen.actionsClosed, seen.actionsRest);
  assert.equal(seen.files, "files");
  assert.equal(seen.closedFiles, null);
  assert.equal(seen.source, "source");
  assert.equal(seen.switchedFiles, "files");
  assert.equal(seen.atEdge, true, "the right sidebar runs from the titlebar's bottom to the window's at its right edge, its views at its top");
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
  assert.equal(seen.draftSearch, "search", "⇧⌘F shows Search");
  assert.equal(seen.draftSourceCount, "1", "Source's icon counts the files it lists");
  assert.equal(seen.sourceCountInSearch, "2", "Source's count refreshes while Search is selected");
  assert.equal(seen.dotInSearch, true, "with changes in Source out of view, the right sidebar's toggle has a dot");
  assert.equal(seen.dotOnSource, false, "on Source, no dot");
  assert.equal(seen.searchKeyFocus, "Search", "with its field taking the keyboard");
  assert.equal(seen.draftSearchReplace, false, "a draft making its own copy has no Replace");
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
  for (const field of ["preferenceKept", "besideOpened", "closedKept", "returnClosed", "loadingClosed", "loadedClosed", "updateClosed", "backgroundClosed", "narrowClosed", "rowOpensTab", "rowClosesCard"]) assert.equal(seen[field], true, field);
  for (const field of ["fileTabClosed", "browserTabClosed", "chatReturnClosed", "activitiesUnfolded", "activitiesFoldKept"]) assert.equal(seen[field], true, field);
  assert.equal(seen.manualOpenFocus, "Context");
  assert.equal(seen.manualEscapeFocus, "Context");
  assert.equal(seen.previewSection, true);
  assert.equal(seen.previewCleared, true, "the preview row disappears when previews are cleared (history may remain)");
  assert.equal(seen.previewReturned, true, "the context pill returns when previews arrive");
  assert.equal(seen.draftFiles, "files");
  assert.match(String(seen.draftTree), /src.*package\.json/);
  assert.equal(seen.draftComposer, "@package.json ");
  assert.deepEqual(seen.draftMention, [{ type: "file", path: "package.json" }]);
  // A new worktree (the default) makes its own copy, so the draft's project folder only reads.
  assert.ok(!(seen.draftSource as string[]).some((label) => /Stage|Discard/.test(label ?? "")), "a draft only reads");
  assert.equal(seen.draftCommitBox, false);
  assert.equal(seen.draftNote, true);
  assert.equal(seen.closedSource, null);
  assert.match(String(seen.worktreeNote), /own copy/, "reopened, it still says why");
  assert.ok(!(seen.worktreeSource as string[]).some((label) => /Stage|Discard/.test(label ?? "")));
  assert.equal(seen.worktreeCommitBox, false);
  // Working in the local checkout, the draft's Source stages and commits in the project folder.
  assert.ok((seen.localSource as string[]).includes("Stage All Changes"), "a local draft stages in the project folder");
  assert.equal(seen.localCommitBox, true);
  assert.equal(seen.chatPane, null);
  assert.match(String(seen.chatText), /Nothing here yet/);
  assert.doesNotMatch(String(seen.chatText), /Plan|Workers/);
  assert.equal(seen.contextClosed, "false");
  assert.equal(seen.contextOpened, "true");
  assert.match(String(seen.chatUsage), /1,200 tokens used/);
  assert.doesNotMatch(String(seen.chatUsage), /Plan|Workers/);
  assert.equal((seen.savedPane as { state: { open: boolean } }).state.open, true);
});
