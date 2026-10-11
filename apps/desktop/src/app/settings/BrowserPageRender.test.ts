import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

const INTRO = "Choose what to bring over to Brigadier's browser.";
const ONCE = "A one-time copy. Passwords and extensions aren't brought over.";
const KEYCHAIN =
  "macOS will ask whether Brigadier may use “Google Chrome Safe Storage” from your Keychain: the key Google Chrome locks its cookies with. Choose Allow. Brigadier uses it only to copy the cookies.";

/** A settings write as the daemon got it: the fields this page owns. */
const settings = (browserEnabled: boolean) => ({
  updateSettings: { browserEnabled, webLinksIn: "brigadier", localLinksIn: "brigadier", browserFullUrl: false },
});

// The real page in Chromium over a faked shell: its rows, where links open, Import… from each
// kind of browser (one open, one macOS keeps locked, one behind the Keychain, one that just
// copies), Clear browsing data, the history and its filter, downloads, site settings, and off.
test("browser settings: rows, import, clear, history, downloads, sites, off", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "browser.html?drive=1", "browser-result", 20000)) as Record<string, unknown>;
  assert.deepEqual(seen.on, {
    title: "Browser",
    sections: ["General", "Downloads", "Browser permissions"],
    rows: [
      "Browser",
      "Open web links in",
      "Open local dev URLs in",
      "Show full URL",
      "Browsing data",
      "Browsing history",
      "Location",
      "Ask where to save downloads",
      "Download history",
      "Site settings",
    ],
  });
  assert.equal(seen.importButton, true);
  // Web links open in the default browser and local ones in Brigadier's, until changed.
  assert.deepEqual(seen.links, ["Your default browser", "Brigadier's browser"]);
  assert.deepEqual(seen.linkChoices, ["Brigadier's browser", "Your default browser"]);
  assert.equal(seen.webLinksIn, "brigadier");

  // Import starts on a browser that isn't open, and says what macOS will ask.
  assert.deepEqual(seen.importOpen, {
    title: "Import from your browser",
    from: "Google Chrome — Person 1",
    boxes: [
      { label: "Cookies", checked: true, disabled: false },
      { label: "Browsing history", checked: true, disabled: false },
    ],
  });
  assert.deepEqual(seen.importNotes, [INTRO, KEYCHAIN, ONCE]);
  assert.deepEqual(seen.profiles, [
    "Dia — DefaultOpen now: quit it first",
    "Google Chrome — Person 1",
    "Firefox — default-release",
    "Safari — Default",
  ]);
  assert.deepEqual(seen.running, {
    notes: [INTRO, "Quit Dia first, so everything copies as it is. Check again", ONCE],
    importable: false,
  });
  const locked = seen.locked as { notes: string[]; settings: boolean; importable: boolean };
  assert.match(locked.notes[1] ?? "", /^To copy from Safari, allow Brigadier in System Settings › Privacy (&amp;|&) Security › Full Disk Access, then open this again\.$/);
  assert.equal(locked.settings, true);
  assert.equal(locked.importable, false);
  assert.deepEqual(seen.chromium, { notes: [INTRO, KEYCHAIN, ONCE], importable: true });
  // A Firefox profile needs no prompt; history alone is imported, and the result says so.
  assert.deepEqual(seen.gecko, [INTRO, ONCE]);
  assert.deepEqual(seen.imported, [INTRO, "Imported 5,321 pages of history from Firefox."]);

  // Clear browsing data: every kind but site settings picked, the sites counted.
  assert.deepEqual(
    (seen.clearKinds as { label: string; checked: boolean }[]).map(({ label, checked }) => [label, checked]),
    [
      ["Browsing history", true],
      ["Cookies and site data", true],
      ["Cached images and files", true],
      ["Download history", true],
      ["Site settings", false],
    ],
  );
  assert.equal((seen.clearNotes as string[])[1], "Kept for 87 sites now. Signs you out of most of them.");
  assert.equal(seen.clearClosed, true);

  const history = seen.history as { title: string; sections: string[]; visits: string[] };
  assert.equal(history.title, "Browsing history");
  assert.deepEqual(history.visits, [
    "http://localhost:5173/checkout",
    "https://www.rust-lang.org/learn",
    "https://developer.apple.com/documentation/webkit",
  ]);
  assert.deepEqual(seen.filters, ["All", "Agent", "Other"]);
  assert.deepEqual(seen.agentVisits, ["http://localhost:5173/checkout"]);
  assert.deepEqual(seen.downloads, {
    title: "Download history",
    files: ["/Users/me/Downloads/report.pdf", "/Users/me/Downloads/big.zip"],
  });
  assert.deepEqual(seen.sites, { title: "Site settings", choices: ["Ask", "Block"] });

  // Off, only the switch and a note are left.
  assert.deepEqual(seen.off, {
    title: "Browser",
    sections: [],
    rows: ["Browser"],
    enabled: false,
    note: [
      "The built-in browser, where links open, and what it keeps.",
      "The built-in browser is off: links open in your default browser, and agents are told it isn't there.",
    ],
  });

  // What reached the shell and the daemon, in order.
  assert.deepEqual(seen.asked, [
    settings(true),
    { import: { id: "firefox:default-release", cookies: false, history: true } },
    { clear: { sinceMs: null, history: true, cookies: true, cache: false, downloads: true, sites: false } },
    { history: "all" },
    { history: "agent" },
    settings(false),
  ]);
});
