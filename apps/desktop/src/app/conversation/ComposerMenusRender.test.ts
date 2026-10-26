import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

test("the @ menu has a heading per kind before a query and one ranked list with one, folders mention, it opens on new chats and sessions; / opens where a word starts and types the CLI's commands", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "composer-menus.html", "composer-menus-result", 90000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined, String(seen.error));
  for (const mode of ["rich", "plain"]) {
    const run = seen[mode] as Record<string, unknown>;
    // No workers here: Files has only its note, the top folders, the other chats.
    assert.deepEqual(run.empty, ["# Files", "~ Type to search for files", "# Folders", "docs", "src", "# Chats", "Release notes"], mode);
    // One list for a query, no headings: the folder whose name matches first, then the files in it.
    assert.deepEqual(run.query, ["lib", "menu.ts", "chips.ts"], mode);
    assert.deepEqual(run.sent, { text: "@src/lib now", mentions: [{ type: "folder", path: "src/lib" }] }, mode);
    assert.equal(run.slashAfterSpaces, true, mode);
    assert.equal(run.slashMidSentence, true, mode);
    assert.equal(run.picked, "a /grill-me", mode);
    assert.equal(run.pickedInside, "z /grill-me tail", mode);
    assert.equal(run.slashInWord, false, mode);
    // A new chat mentions the other conversations; a new session its project's files.
    assert.deepEqual(run.newChat, ["# Chats", "Menus", "Release notes"], mode);
    assert.equal(run.newSession, true, mode);
    assert.equal(run.sessionFusion, false, mode);
    assert.equal(run.fusionDisabled, true, mode);
    assert.equal(run.fusionHint, true, mode);
    assert.equal(run.fusionOff, true, mode);
    assert.deepEqual(run.fusionSend, { fusion: true, text: "Compare the options." }, mode);
    assert.equal(run.fusionPill, "Fusion", mode);
    assert.equal(run.rawFusion, false, mode);
    assert.equal(run.fusionSlashRow, false, mode);
  }
});
