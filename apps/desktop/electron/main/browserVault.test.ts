import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { fake } from "../testing/fakeElectron.ts";

(globalThis as { BRIGADIER_PRODUCTION?: boolean }).BRIGADIER_PRODUCTION = false;
const { importLogins, loginSite, profileFile, putLogin, readVault, replaceLogins, vaultRefusal, writeVault } = await import("./browserVault.ts");

fake.userData = fs.mkdtempSync(path.join(os.tmpdir(), "brigadier-vault-test-"));
test.after(() => fs.rmSync(fake.userData, { recursive: true, force: true }));

const vaultOf = (profile: string) => profileFile(profile, "Brigadier Vault");
const emptyVault = () => ({ nextId: 1, logins: [], never: [] });

function onPlatform(t: { after: (done: () => void) => void }, platform: string): void {
  const real = Object.getOwnPropertyDescriptor(process, "platform")!;
  Object.defineProperty(process, "platform", { value: platform });
  t.after(() => Object.defineProperty(process, "platform", real));
}

test("a login's site is the origin of an http or https address, and nothing else", () => {
  for (const [url, site] of [
    ["https://example.test/login?next=/home#top", "https://example.test"],
    ["http://127.0.0.1:4677/spa", "http://127.0.0.1:4677"],
    ["HTTPS://EXAMPLE.test", "https://example.test"],
    ["https://sam:secret@example.test/", "https://example.test"],
    ["ftp://example.test/", null],
    ["file:///etc/hosts", null],
    ["javascript:alert(1)", null],
    ["about:blank", null],
    ["example.test", null],
    ["", null],
  ] as const) assert.equal(loginSite(url), site, url);
});

test("saving a login adds, updates or leaves it, by site and username", () => {
  const vault = emptyVault();
  assert.equal(putLogin(vault, "https://a.test", "sam", "fake-one", 10), "added");
  assert.deepEqual(vault, {
    nextId: 2,
    never: [],
    logins: [{ id: 1, site: "https://a.test", username: "sam", password: "fake-one", createdMs: 10, usedMs: 10 }],
  });

  assert.equal(putLogin(vault, "https://a.test", "sam", "fake-one", 20), "unchanged");
  assert.equal(vault.logins[0]!.usedMs, 10, "the same password touches nothing");

  assert.equal(putLogin(vault, "https://a.test", "sam", "fake-two", 30), "updated");
  assert.deepEqual(vault.logins[0], { id: 1, site: "https://a.test", username: "sam", password: "fake-two", createdMs: 10, usedMs: 30 });

  // Another username or another site is another login.
  assert.equal(putLogin(vault, "https://a.test", "kim", "fake-two", 40), "added");
  assert.equal(putLogin(vault, "https://b.test", "sam", "fake-two", 50), "added");
  assert.equal(putLogin(vault, "https://a.test", "", "fake-three", 60), "added");
  assert.deepEqual(vault.logins.map(({ id }) => id), [1, 2, 3, 4]);
});

test("a saved vault is sealed on disk, and reads back", () => {
  writeVault("sealed", { nextId: 2, logins: [{ id: 1, site: "https://a.test", username: "fake-sam", password: "fake-Tulip-41", createdMs: 1, usedMs: 1 }], never: [] });
  const onDisk = fs.readFileSync(vaultOf("sealed"), "utf8");
  assert.equal(onDisk.includes("fake-Tulip-41"), false);
  assert.equal(onDisk.includes("fake-sam"), false);
  assert.equal(readVault("sealed").logins[0]!.password, "fake-Tulip-41");
  assert.equal(path.basename(path.dirname(vaultOf("sealed"))), "browser-profile-sealed", "it lives in the profile's folder");
  assert.deepEqual(fs.readdirSync(path.dirname(vaultOf("sealed"))), ["Brigadier Vault"], "no temporary file stays");
});

test("a profile with no vault reads as empty", () => {
  assert.deepEqual(readVault("never-used"), emptyVault());
});

test("importing logins counts each row, and removes nothing", async () => {
  const profile = "import-counts";
  await importLogins(profile, [
    { url: "https://stays.test/", username: "old", password: "fake-old" },
    { url: "https://changes.test/", username: "sam", password: "fake-old" },
  ]);
  const before = readVault(profile).logins.find((login) => login.site === "https://changes.test")!;

  const counts = await importLogins(profile, [
    { url: "https://new.test/login", username: "sam", password: "fake-new" },
    { url: "https://changes.test/any/page", username: "sam", password: "fake-new" },
    { url: "https://stays.test/other", username: "old", password: "fake-old" },
    { url: "ftp://bad.test/", username: "sam", password: "fake-new" },
    { url: "not an address", username: "sam", password: "fake-new" },
    { url: "https://nopassword.test/", username: "sam", password: "" },
  ]);
  assert.deepEqual(counts, { added: 1, updated: 1, unchanged: 1, skipped: 3 });

  const logins = readVault(profile).logins;
  assert.deepEqual(logins.map(({ site }) => site), ["https://stays.test", "https://changes.test", "https://new.test"]);
  const changed = logins[1]!;
  assert.equal(changed.password, "fake-new");
  assert.equal(changed.id, before.id);
});

test("an import that changes nothing writes nothing", async () => {
  const counts = await importLogins("import-nothing", [{ url: "ftp://bad.test/", username: "", password: "fake-new" }]);
  assert.deepEqual(counts, { added: 0, updated: 0, unchanged: 0, skipped: 1 });
  assert.equal(fs.existsSync(vaultOf("import-nothing")), false);
});

test("replacing logins keeps what stays, its id and dates, and the never list", async () => {
  const profile = "replace-counts";
  writeVault(profile, {
    nextId: 4,
    never: ["https://never.test"],
    logins: [
      { id: 1, site: "https://stays.test", username: "sam", password: "fake-same", createdMs: 100, usedMs: 200 },
      { id: 2, site: "https://changes.test", username: "sam", password: "fake-old", createdMs: 100, usedMs: 200 },
      { id: 3, site: "https://goes.test", username: "sam", password: "fake-gone", createdMs: 100, usedMs: 200 },
    ],
  });

  const counts = await replaceLogins(profile, [
    { url: "https://stays.test/login", username: "sam", password: "fake-same" },
    { url: "https://changes.test/login", username: "sam", password: "fake-new" },
    { url: "https://new.test/login", username: "sam", password: "fake-new" },
    { url: "https://new.test/again", username: "sam", password: "fake-repeat" },
    { url: "ftp://bad.test/", username: "sam", password: "fake-new" },
    { url: "https://nopassword.test/", username: "sam", password: "" },
  ]);
  assert.deepEqual(counts, { added: 1, updated: 1, unchanged: 1, skipped: 3 });

  const vault = readVault(profile);
  assert.deepEqual(vault.never, ["https://never.test"]);
  assert.deepEqual(vault.logins.map(({ id, site, password }) => [id, site, password]), [
    [1, "https://stays.test", "fake-same"],
    [2, "https://changes.test", "fake-new"],
    [4, "https://new.test", "fake-new"],
  ]);
  assert.equal(vault.logins[0]!.usedMs, 200);
  assert.equal(vault.logins[1]!.createdMs, 100);
  assert.notEqual(vault.logins[1]!.usedMs, 200);
  assert.equal(vault.nextId, 5);
});

test("replacing with nothing usable empties the list: section 5 guards this", async () => {
  const profile = "replace-empty";
  await importLogins(profile, [{ url: "https://a.test/", username: "sam", password: "fake-one" }]);
  const counts = await replaceLogins(profile, [{ url: "ftp://bad.test/", username: "sam", password: "fake-one" }]);
  assert.deepEqual(counts, { added: 0, updated: 0, unchanged: 0, skipped: 1 });
  assert.deepEqual(readVault(profile).logins, []);
});

test("a vault that can't be opened throws, and nothing overwrites it", async () => {
  const profile = "unreadable";
  const file = vaultOf(profile);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, "sealed with a key this computer lost");

  assert.throws(() => readVault(profile), /decrypting/);
  const row = { url: "https://a.test/", username: "sam", password: "fake-one" };
  await assert.rejects(importLogins(profile, [row]), /decrypting/);
  await assert.rejects(replaceLogins(profile, [row]), /decrypting/);
  assert.equal(fs.readFileSync(file, "utf8"), "sealed with a key this computer lost");
});

test("a vault that opens to something other than JSON throws too", () => {
  const profile = "not-json";
  const file = vaultOf(profile);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, Buffer.from(`fake-sealed:${Buffer.from("not json").toString("base64")}`));
  assert.throws(() => readVault(profile), SyntaxError);
});

test("no safe store: nothing reads or writes", (t) => {
  t.after(() => { fake.encryption = true; });
  fake.encryption = false;
  assert.match(vaultRefusal()!, /safe store isn't available/);
  assert.throws(() => readVault("refused"), /safe store isn't available/);
  assert.throws(() => writeVault("refused", emptyVault()), /safe store isn't available/);
  assert.equal(fs.existsSync(vaultOf("refused")), false);
});

test("Linux's basic_text is refused, since its fixed key protects nothing", (t) => {
  t.after(() => { fake.backend = "gnome_libsecret"; });
  onPlatform(t, "linux");
  fake.backend = "basic_text";
  assert.match(vaultRefusal()!, /No system keyring/);
  assert.throws(() => writeVault("refused-linux", emptyVault()), /No system keyring/);
  assert.equal(fs.existsSync(vaultOf("refused-linux")), false);

  fake.backend = "gnome_libsecret";
  assert.equal(vaultRefusal(), null);
});

test("only Linux reads the backend's name", (t) => {
  t.after(() => { fake.backend = "gnome_libsecret"; });
  onPlatform(t, "darwin");
  fake.backend = "basic_text";
  assert.equal(vaultRefusal(), null);
});
