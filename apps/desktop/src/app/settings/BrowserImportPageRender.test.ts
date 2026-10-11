import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

const n = (value: number) => new Intl.NumberFormat().format(value);
const OFF = "The built-in browser is off, so there is nothing to import into. Turn it on in Settings › Browser.";
const DESCRIPTION =
  "Bring your sign-ins from another browser on this Mac into Brigadier's browser, so previews and agent tabs open already signed in.";
const FDA =
  "Safari keeps its cookies in a protected folder. Turn on Full Disk Access for Brigadier in System Settings › Privacy & Security, then come back. You can turn it off again after the import.";
const toast = (tone: string, text: string) => ({ tone, text });
const asked = (key: string, ids: string[]) => ({ import: { key, ids, history: true } });
const row = (name: string, status: string, details: string[], button: string | null, enabled: boolean, icon = "globe", badge: string | null = null) => ({
  name,
  badge,
  status,
  details,
  button,
  enabled,
  icon,
});


// The real page in Chromium over a faked shell: a row for each state a browser can be in, the
// profile picker and its count, an import that can't be walked away from, Full Disk Access,
// blocked imports and what puts them right, the toasts and the last import's record, the
// browser off, and no browsers or an error.
test("import browser data: rows, picker, importing, access, blocked, toasts, record", { timeout: 90000 }, async (t) => {
  const dumped = await renderFixturePage(t, "browser-import.html?drive=1", "browser-import-result", 30000);
  const seen = JSON.parse(dumped.replaceAll("&amp;", "&")) as Record<string, unknown>;

  assert.deepEqual(seen.page, { title: "Import Browser Data", section: "Browsers", description: DESCRIPTION, refresh: true });
  assert.deepEqual(seen.rows, [
    row("Google Chrome", "Running · quit Google Chrome to import", [], "Recheck", true, "app"),
    row("Helium", "Ready", ["1 profile", "40 cookies"], "Import…", true, "app"),
    row("Microsoft Edge", "Needs Keychain access", [], "Import…", true),
    row("Brave", "No encryption key in Keychain", [], "Recheck", true, "app"),
    row("Vivaldi", "Unavailable", [], null, false),
    row(
      "Arc",
      "Ready",
      [`Arc · ${n(887)} cookies imported`, "26 skipped (accounts.google.com, github.com, x.com and 1 more)"],
      "Import…",
      true,
      "globe",
      "Imported 3d ago",
    ),
    row("Dia", "Ready", ["2 profiles", `${n(4045)} cookies`], "Import…", true, "app"),
    row("Firefox", "No cookies yet", ["1 profile"], "Import…", false),
    row("Safari", "Needs Full Disk Access", [], "Grant access…", true, "app"),
  ]);

  // Several profiles: the picker, the default ticked and counted, history on.
  assert.deepEqual(seen.choose, {
    step: "choose",
    title: "Import from Dia",
    description: "Which profiles should be copied into Brigadier's browser?",
    notes: [
      "Which profiles should be copied into Brigadier's browser?",
      "Dia must stay closed during the import. macOS may ask for Keychain access to its encryption key; choose Allow (Always Allow avoids asking again).",
    ],
    buttons: ["Cancel", `Import ${n(2841)} cookies`],
    closable: true,
    profiles: [
      { name: "Personal", cookies: `${n(2841)} cookies`, checked: true },
      { name: "Work", cookies: `${n(1204)} cookies`, checked: false },
    ],
  });
  assert.equal(seen.historyBox, "checked");
  // The count follows the ticks; with none ticked there is nothing to import.
  assert.deepEqual((seen.bothTicked as { buttons: string[] }).buttons, ["Cancel", `Import ${n(4045)} cookies`]);
  assert.deepEqual(seen.workOnly, ["Cancel", `Import ${n(1204)} cookies`]);
  assert.deepEqual(seen.noneTicked, ["Cancel", "Import 0 cookies (off)"]);
  assert.equal(seen.historyOff, "unchecked");

  // Importing: the counter, no buttons and no close button; Esc and a click outside leave it be,
  // and every row waits.
  assert.deepEqual(seen.importing, {
    step: "importing",
    title: "Importing from Dia…",
    description: `Adding cookies… ${n(1200)} of ${n(4045)}`,
    notes: [`Adding cookies… ${n(1200)} of ${n(4045)}`],
    buttons: [],
    closable: false,
    rowsEnabled: [],
  });
  assert.equal(seen.stillImporting, "importing");
  // Done: the dialog goes, a toast says what came over, and the row keeps the record.
  assert.deepEqual(seen.imported, {
    dialog: null,
    toasts: [toast("success", `Imported ${n(3994)} cookies from Dia`)],
    dia: row(
      "Dia",
      "Ready",
      [`Personal, Work · ${n(3994)} cookies imported`, "51 skipped (accounts.google.com and mail.google.com)"],
      "Import…",
      true,
      "app",
      "Imported just now",
    ),
  });

  // Full Disk Access: still off after "I've turned it on", then on, which imports Safari's one
  // profile straight away.
  const fda = { step: "fullDiskAccess", title: "Allow Full Disk Access for Safari", description: FDA, buttons: ["Cancel", "Open System Settings", "I've turned it on"], closable: true };
  assert.deepEqual(seen.fda, { ...fda, notes: [FDA] });
  assert.deepEqual(seen.fdaStillOff, {
    ...fda,
    notes: [FDA, "Full Disk Access is still off. macOS may need Brigadier to quit and reopen before the change applies."],
  });
  assert.deepEqual((seen.fdaOn as { dialog: unknown }).dialog, null);
  assert.deepEqual((seen.fdaOn as { toasts: unknown[] }).toasts.at(-1), toast("success", `Imported 12 cookies and 340 pages of history from Safari`));

  // Blocked: the Keychain refused, then a browser that was open, quit, and imported.
  const keychain =
    "Brigadier needs Keychain access to read Microsoft Edge's cookie encryption key. Choose Allow when macOS asks, then try again.";
  assert.deepEqual(seen.keychain, {
    step: "blocked",
    title: "Can't import from Microsoft Edge",
    description: keychain,
    notes: [keychain],
    buttons: ["Close", "Try again"],
    closable: true,
  });
  assert.equal(seen.keychainClosed, true);
  assert.deepEqual(seen.running, {
    step: "blocked",
    title: "Can't import from Helium",
    description: "Quit Helium first, so its cookies can be read.",
    notes: ["Quit Helium first, so its cookies can be read."],
    buttons: ["Close", "I've quit it"],
    closable: true,
  });
  assert.deepEqual((seen.quit as { toasts: unknown[] }).toasts.at(-1), toast("success", "No cookies were imported from Helium"));
  // A read that fails ends in an error toast, with no dialog.
  assert.deepEqual(seen.readFailed, {
    dialog: null,
    toasts: [
      toast("success", `Imported ${n(3994)} cookies from Dia`),
      toast("success", "Imported 12 cookies and 340 pages of history from Safari"),
      toast("success", "No cookies were imported from Helium"),
      toast("error", "Arc: Arc's cookies couldn't be read (database disk image is malformed)."),
    ],
  });
  assert.equal(seen.rechecked, true);

  // Off, nothing can be imported into: every button waits and a line says why.
  assert.deepEqual(seen.off, { note: OFF, enabled: [] });
  assert.deepEqual(seen.empty, [DESCRIPTION, "No supported browsers were found on this machine.", OFF]);
  assert.deepEqual(seen.error, [DESCRIPTION, "Could not check the installed browsers.", OFF]);

  // What reached the shell, in order: the ticked profiles with history, System Settings, then
  // each one-profile import.
  assert.deepEqual(seen.asked, [
    asked("dia", ["dia/Default", "dia/Profile 1"]),
    "openAccess",
    asked("safari", ["safari/Default"]),
    asked("edge", ["edge/Default"]),
    asked("helium", ["helium/Default"]),
    asked("helium", ["helium/Default"]),
    asked("arc", ["arc/Default"]),
  ]);
});
