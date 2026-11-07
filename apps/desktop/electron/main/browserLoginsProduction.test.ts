import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { fake } from "../testing/fakeElectron.ts";

// A production build, with everything a dev run's stand-in would need set: it must make no difference.
(globalThis as { BRIGADIER_PRODUCTION?: boolean }).BRIGADIER_PRODUCTION = true;
process.env.BRIGADIER_DEV = "1";
process.env.BRIGADIER_DEV_SYSTEM_CHECK = "pass";
const { installBrowserVault } = await import("./browserLogins.ts");
const { dev, rendererUrl } = await import("./identity.ts");

fake.userData = fs.mkdtempSync(path.join(os.tmpdir(), "brigadier-logins-production-test-"));
test.after(() => fs.rmSync(fake.userData, { recursive: true, force: true }));
installBrowserVault();

const real = Object.getOwnPropertyDescriptor(process, "platform")!;
Object.defineProperty(process, "platform", { value: "darwin" });
test.after(() => Object.defineProperty(process, "platform", real));

const vault = (op: string, args: object) =>
  fake.handlers.get("brigadier:vault")!({ sender: { id: 1, getType: () => "window" }, senderFrame: { parent: null, url: rendererUrl } }, op, args);

const profile = "production";
vault("add", { profile, site: "https://site.test", username: "fake-sam", password: "fake-Tulip-41" });
const reveal = () => vault("reveal", { profile, id: 1 });

test("a production build isn't a dev run, whatever the environment says", () => {
  assert.equal(dev, false);
});

test("the stand-in is off: a Mac with no Touch ID shows nothing", async () => {
  fake.canPromptTouchID = false;
  assert.deepEqual(await reveal(), { kind: "unavailable" });
  assert.deepEqual(fake.touchIdAsked, []);
});

test("the stand-in is off: cancelling Touch ID shows nothing", async () => {
  fake.canPromptTouchID = true;
  fake.touchIdAnswer = false;
  assert.deepEqual(await reveal(), { kind: "declined" });
});

test("Touch ID passing shows the password, and says which site asks", async () => {
  fake.canPromptTouchID = true;
  fake.touchIdAnswer = true;
  fake.touchIdAsked.length = 0;
  assert.deepEqual(await reveal(), { kind: "shown", password: "fake-Tulip-41" });
  assert.deepEqual(fake.touchIdAsked, ["show your password for site.test"]);
});
