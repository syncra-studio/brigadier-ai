import assert from "node:assert/strict";
import { test } from "node:test";

import type { ImportRecord, ImportSource } from "@/ipc/generated";
import {
  doneToast,
  doneTone,
  domainsText,
  importedBadge,
  opensDialog,
  progressText,
  reasonText,
  retryable,
  rowView,
} from "@/state/browserImport";

const n = (value: number) => new Intl.NumberFormat().format(value);

const source = (fields: Partial<ImportSource> = {}): ImportSource => ({
  key: "dia",
  name: "Dia",
  family: "chromium",
  icon: null,
  state: "ready",
  profiles: [
    { id: "dia/Default", name: "Personal", cookies: 2841, isDefault: true },
    { id: "dia/Profile 1", name: "Work", cookies: 1204, isDefault: false },
  ],
  record: null,
  ...fields,
});

const record = (fields: Partial<ImportRecord> = {}): ImportRecord => ({
  atMs: 0,
  profiles: ["Work"],
  imported: 2790,
  skipped: 51,
  skippedDomains: ["accounts.google.com", "mail.google.com"],
  skippedDomainCount: 2,
  history: 0,
  ...fields,
});

test("each state gives its row a status, a tone and one button", () => {
  const view = (fields: Partial<ImportSource>) => {
    const { status, tone, action, label } = rowView(source(fields));
    return [status, tone, action, label];
  };
  assert.deepEqual(view({}), ["Ready", "ready", "import", "Import…"]);
  assert.deepEqual(view({ state: "running", profiles: [] }), ["Running · quit Dia to import", "attention", "recheck", "Recheck"]);
  assert.deepEqual(view({ state: "needsFullDiskAccess", profiles: [] }), ["Needs Full Disk Access", "attention", "grant", "Grant access…"]);
  assert.deepEqual(view({ state: "needsKeychainAccess" }), ["Needs Keychain access", "attention", "import", "Import…"]);
  assert.deepEqual(view({ state: "noKeychainKey", profiles: [] }), ["No encryption key in Keychain", "idle", "recheck", "Recheck"]);
  assert.deepEqual(view({ state: "unavailable", profiles: [] }), ["Unavailable", "idle", "none", null]);
  assert.deepEqual(view({ profiles: [] }), ["No cookies yet", "idle", "none", "Import…"]);
  assert.deepEqual(rowView(source({ profiles: [{ id: "f/a", name: "a", cookies: 0, isDefault: true }] })).details, ["1 profile"]);
});

test("a ready row counts profiles and cookies, or says what the last import brought", () => {
  assert.deepEqual(rowView(source()).details, ["2 profiles", `${n(4045)} cookies`]);
  // A profile that couldn't be counted leaves the total out.
  assert.deepEqual(
    rowView(source({ profiles: [{ id: "a", name: "a", cookies: null, isDefault: true }] })).details,
    ["1 profile"],
  );
  assert.deepEqual(rowView(source({ record: record() })).details, [
    `Work · ${n(2790)} cookies imported`,
    "51 skipped (accounts.google.com and mail.google.com)",
  ]);
  assert.deepEqual(
    rowView(source({ record: record({ profiles: ["Work", "HostPapa"], skipped: 0 }) })).details,
    [`Work, HostPapa · ${n(2790)} cookies imported`],
  );
});

test("skipped domains name three, then how many more in all", () => {
  assert.equal(domainsText([], 0), "");
  assert.equal(domainsText(["a.com"], 1), "a.com");
  assert.equal(domainsText(["a.com", "b.com", "c.com"], 3), "a.com, b.com and c.com");
  const sample = Array.from({ length: 20 }, (_, i) => `s${i}.com`);
  assert.equal(domainsText(sample, 52), "s0.com, s1.com, s2.com and 49 more");
});

test("the badge says how long ago", () => {
  const now = 10 * 86_400_000;
  assert.equal(importedBadge(record({ atMs: now - 10_000 }), now), "Imported just now");
  assert.equal(importedBadge(record({ atMs: now - 3 * 86_400_000 }), now), "Imported 3d ago");
});

test("the toast after an import says what came over", () => {
  assert.equal(doneToast("Dia", record()), `Imported ${n(2790)} cookies from Dia`);
  assert.equal(doneToast("Dia", record({ imported: 1, history: 120 })), "Imported 1 cookie and 120 pages of history from Dia");
  assert.equal(doneToast("Firefox", record({ imported: 0, history: 3 })), "Imported 3 pages of history from Firefox");
  assert.equal(doneToast("Arc", record({ imported: 0 })), "No cookies were imported from Arc");
  assert.deepEqual(
    [record(), record({ imported: 0, history: 3 }), record({ imported: 0 })].map(doneTone),
    ["success", "success", "neutral"],
  );
});

test("failures a person can put right open a dialog; the rest end in a toast", () => {
  assert.deepEqual(
    (["running", "needsFullDiskAccess", "needsKeychainAccess", "noKeychainKey", "gone", "readFailed"] as const).filter(opensDialog),
    ["running", "needsFullDiskAccess", "needsKeychainAccess", "noKeychainKey"],
  );
  assert.equal(retryable("gone"), false);
  assert.equal(retryable("readFailed"), true);
  assert.equal(reasonText("running", "Chrome"), "Quit Chrome first, so its cookies can be read.");
  assert.equal(
    reasonText("needsKeychainAccess", "Dia"),
    "Brigadier needs Keychain access to read Dia's cookie encryption key. Choose Allow when macOS asks, then try again.",
  );
});

test("progress reads, then counts the cookies added, then history", () => {
  assert.equal(progressText(null), "Reading and decrypting cookies. macOS may ask for Keychain access.");
  assert.equal(progressText({ type: "cookies", done: 1200, total: 2841 }), `Adding cookies… ${n(1200)} of ${n(2841)}`);
  assert.equal(progressText({ type: "history" }), "Adding history…");
});
