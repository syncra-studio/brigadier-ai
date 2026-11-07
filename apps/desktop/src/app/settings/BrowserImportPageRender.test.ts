import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

const n = (value: number) => new Intl.NumberFormat().format(value);
const OFF = "The built-in browser is off, so there is nothing to import into. Turn it on in Settings › Browser.";
const DESCRIPTION =
  "Bring your profiles from another browser on this computer. Each one becomes its own profile here, so previews and agent tabs open already signed in.";
const FDA =
  "Safari keeps its cookies in a protected folder. Turn on Full Disk Access for Brigadier in System Settings › Privacy & Security, then come back. You can turn it off again after the import.";
const ALL = { signIns: true, bookmarks: true, passwords: true };
const asked = (key: string, ids: string[]) => ({ import: { key, ids, history: true, ...ALL, replace: false } });
const PICK_NOTE = (name: string) => `Close ${name} first. Your computer may ask you to allow access to its saved data. Passkeys aren't copied: they stay on your device, and on this Mac they can't be used here yet.`;
const REPLACING = "Replacing swaps a profile's sign-ins and passwords for these. Its bookmarks and history are combined.";
const noPasswords = (key: string, ids: string[]) => ({ import: { key, ids, history: true, ...ALL, passwords: false, replace: false } });
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
// profile picker and its count, an import that can't be walked away from, the results per profile
// with a retried failure, Full Disk Access, blocked imports and what puts them right, the last
// import's record, the browser off, and no browsers or an error.
test("import browser data: rows, picker, importing, results, access, blocked, record", { timeout: 90000 }, async (t) => {
  const dumped = await renderFixturePage(t, "browser-import.html?drive=1", "browser-import-result", 30000);
  const seen = JSON.parse(dumped.replaceAll("&amp;", "&")) as Record<string, unknown>;

  assert.deepEqual(seen.page, { title: "Import Browser Data", section: "Browsers", description: DESCRIPTION, refresh: true });
  assert.deepEqual(seen.rows, [
    row("Google Chrome", "Running · quit Google Chrome to import", [], "Recheck", true, "app"),
    row("Chromium", "Ready", ["1 profile", "12 cookies"], "Import…", true),
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
    row("Firefox", "Ready", ["1 profile", "0 cookies"], "Import…", true),
    row("Safari", "Needs Full Disk Access", [], "Grant access…", true, "app"),
  ]);

  // The picker: each profile new and ticked, all the parts, the count following the ticks.
  assert.deepEqual(seen.choose, {
    step: "choose",
    title: "Import from Dia",
    description: "Each profile you tick stays separate in Brigadier.",
    notes: [
      "Each profile you tick stays separate in Brigadier.",
      REPLACING,
      "Skipped Guest: nothing to bring.",
      "Agents working in a profile use its sign-ins. To keep agents out of a site, keep it in another profile.",
      PICK_NOTE("Dia"),
    ],
    buttons: ["New profile", "New profile", "Cancel", "Import 2 profiles"],
    closable: true,
    profiles: [
      { name: "Personal", line: "Not imported yet", into: "New profile", checked: true },
      { name: "Work", line: "Not imported yet", into: "New profile", checked: true },
    ],
  });
  assert.deepEqual(seen.bring, ["checked", "checked", "checked", "checked"]);
  assert.deepEqual(seen.personalOnly, ["Cancel", "Import 1 profile"]);
  assert.deepEqual(seen.noneTicked, ["Cancel", "Import 0 profiles (off)"]);
  assert.equal(seen.bookmarksOff, "unchecked");

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

  // The results: per profile and part, a failure with Try again, each site to sign in to again.
  const results = seen.results as { step: string; title: string; description: string; buttons: string[]; profiles: unknown };
  assert.equal(results.step, "results");
  assert.equal(results.title, "Imported from Dia");
  assert.equal(results.description, "2 profiles are ready. Bookmarks for Work didn't come across. 2 sites need a fresh sign-in.");
  assert.deepEqual(results.buttons, ["Sign in", "Sign in", "Try again", "Open Dia: Personal", "Done"]);
  assert.deepEqual(results.profiles, [
    {
      name: "Dia: Personal",
      badge: "New",
      parts: [
        ["part", `Sign-ins${n(2800)} cookies · 2 sites left out`],
        ["yes", "History900 pages"],
        ["yes", "Bookmarks40 bookmarks"],
        ["yes", "Passwords21 passwords"],
      ],
      sites: ["accounts.google.com · tied to Dia on this deviceSign in", "mail.google.com · its sign-in couldn't be copiedSign in"],
      google: "Google sign-ins aren't imported: sign in once here",
    },
    {
      name: "Work",
      badge: "New",
      parts: [
        ["yes", `Sign-ins${n(1194)} cookies`],
        ["yes", "History300 pages"],
        ["no", "BookmarksDia's bookmarks couldn't be readTry again"],
        ["part", "PasswordsNone from Dia could be used, so this profile kept its own"],
      ],
      sites: [],
      google: "",
    },
  ]);
  assert.deepEqual(seen.retried, ["yes", "Bookmarks12 bookmarks"]);
  // Done: the dialog goes, and the row keeps the record and names what is here now.
  assert.deepEqual(seen.imported, {
    dialog: null,
    dia: row("Dia", "Ready", [`Personal, Work · ${n(3994)} cookies imported`, "51 skipped (accounts.google.com and mail.google.com)"], "Import…", true, "app", "Imported just now"),
  });

  // Full Disk Access: still off after "I've turned it on", then on, which goes on to the picker.
  const fda = { step: "fullDiskAccess", title: "Allow Full Disk Access for Safari", description: FDA, buttons: ["Cancel", "Open System Settings", "I've turned it on"], closable: true };
  assert.deepEqual(seen.fda, { ...fda, notes: [FDA] });
  assert.deepEqual(seen.fdaStillOff, {
    ...fda,
    notes: [FDA, "Full Disk Access is still off. macOS may need Brigadier to quit and reopen before the change applies."],
  });
  assert.equal(seen.fdaOn, "choose");
  // Passwords that can't be read say why, with no Try again, and the rest still counts.
  assert.deepEqual(seen.safari, {
    description: "1 profile is ready. Passwords for Safari: Default didn't come across.",
    buttons: ["Open Safari: Default", "Done"],
    profiles: [
      {
        name: "Safari: Default",
        badge: "New",
        parts: [
          ["yes", "Sign-ins12 cookies"],
          ["yes", "History340 pages"],
          ["yes", "Bookmarks5 bookmarks"],
          ["no", "PasswordsSafari keeps its passwords in iCloud Keychain, which other apps can't read."],
        ],
        sites: [],
        google: "",
      },
    ],
  });
  assert.deepEqual(seen.firefox, {
    description: "1 profile is ready. Passwords for Firefox: Default didn't come across.",
    buttons: ["Open Firefox: Default", "Done"],
    passwords: ["no", "PasswordsFirefox's passwords are locked with a Primary Password."],
  });
  // A build that can't bring a browser's passwords has no box for them, and doesn't promise them.
  assert.equal(seen.heliumParts, 4, "the profile's and three parts'");
  assert.equal(seen.heliumNote, "Replacing swaps a profile's sign-ins for these. Its bookmarks and history are combined.");

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
  assert.equal(seen.quit, "1 profile is ready.");
  // A read that fails says so in the results, with nothing to open.
  assert.deepEqual(seen.readFailed, {
    description: "Nothing came across. Default couldn't be imported.",
    buttons: ["Done"],
    profiles: [
      {
        name: "Default",
        badge: "New",
        parts: [["no", "EverythingArc's cookies couldn't be read (database disk image is malformed)."]],
        sites: [],
        google: "",
      },
    ],
  });
  assert.equal(seen.madeForFailedRead, false);
  assert.equal(seen.rechecked, true);

  // Off, nothing can be imported into: every button waits and a line says why.
  assert.deepEqual(seen.off, { note: OFF, enabled: [] });
  assert.deepEqual(seen.empty, [DESCRIPTION, "No supported browsers were found on this machine.", OFF]);
  assert.deepEqual(seen.error, [DESCRIPTION, "Could not check the installed browsers.", OFF]);

  // What was written, in order: each ticked profile on its own, the retried part alone, System
  // Settings, then each import whose read worked: Edge's and Arc's stopped at the read.
  assert.deepEqual(seen.asked, [
    asked("dia", ["dia/Default"]),
    asked("dia", ["dia/Profile 1"]),
    { import: { key: "dia", ids: ["dia/Profile 1"], history: false, signIns: false, bookmarks: true, passwords: false, replace: true } },
    "openAccess",
    asked("safari", ["safari/Default"]),
    asked("firefox", ["firefox/Default"]),
    noPasswords("helium", ["helium/Default"]),
  ]);
});
