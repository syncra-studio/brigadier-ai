import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

// The Browser tab in Chromium over a faked shell: the address field's suggestions (search the
// web, switch to tab, history with remove, the arrow keys, Esc), the page's question for the
// camera and microphone (above the live page, each answer once), and the Downloads popover.
test("browser tab tools: address suggestions, the page's question for the camera, downloads", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "browser-tab-tools.html?drive=1", "browser-tab-tools-result", 60000)) as Record<string, unknown>;
  if (process.env.BROWSER_TAB_SEEN) console.log(JSON.stringify(seen, null, 2));

  // Typing "docs": search the web first, the session's other tab whose page matches, then the
  // history's matching pages but the one that tab shows; as wide as the address, which keeps the
  // keyboard.
  const row = (kind: string, text: string, selected = false) => ({ kind, text, selected });
  assert.deepEqual(seen.typed, {
    rows: [
      row("go", "Search the web for “docs”"),
      row("tab", "The guide · docs.example.org/guideSwitch to tab"),
      row("visit", "Docs news · news.test/docs"),
      row("visit", "https://old.test/docs-archive"),
    ],
    expanded: "true",
    focused: true,
    width: 770,
    address: 770,
  });
  assert.deepEqual(seen.arrows, { selected: 1, active: "browser-suggestion-1" });
  assert.deepEqual(seen.switched, { active: "browser:guide", suggestions: false, asked: [] }, "Switch to tab goes to it, nothing loads");
  assert.deepEqual(seen.removed, { rows: ["go", "tab", "visit"], focused: true, asked: [{ removeHistory: [2] }] });
  assert.deepEqual(seen.escape, { suggestions: false, value: "docs" });
  assert.equal(seen.escapeAgain, "https://www.example.com/");
  assert.deepEqual(seen.search, [{ navigate: "https://www.google.com/search?q=cats" }]);

  // The question hangs under the address from its start, above the page and not taking it.
  assert.deepEqual(seen.prompt, {
    text: "meet.example wants toClose permission requestUse your cameraUse your microphoneAllow while visiting the siteAllow this timeNever allow",
    above: true,
    modal: false,
    startAligned: 1,
    below: true,
    buttons: ["Close permission request", "Allow while visiting the site", "Allow this time", "Never allow"],
  });
  assert.equal(seen.promptStays, true, "a click elsewhere leaves the question");
  assert.match(String(seen.microphoneOnly), /wants toClose permission requestUse your microphoneAllow/);
  assert.deepEqual(seen.answers, [
    { permission: 7, answer: "visiting" },
    { permission: 8, answer: "once" },
    { permission: 9, answer: "never" },
    { permission: 10, answer: "dismiss" },
    { permission: 11, answer: "dismiss" },
  ]);

  // Downloads: Open and Show in Finder for a finished file, a failed one says so.
  assert.deepEqual(seen.downloads, ["report.pdffiles.testOpenShow in Finder", "big.zipFailed · files.test"]);
  assert.deepEqual(seen.downloadsAsked, [{ openDownload: 11 }, { reveal: "/Users/me/Downloads/report.pdf" }]);
  assert.deepEqual(seen.allDownloads, { type: "settings", page: "browser" });
});
