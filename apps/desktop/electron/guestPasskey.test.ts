import assert from "node:assert/strict";
import test from "node:test";

import { fake } from "./testing/fakeElectron.ts";

await import("./guestPasskey.ts");
const script = fake.mainWorld[0]!;

type Options = { publicKey?: object; mediation?: string; password?: boolean };
type Credentials = Record<"get" | "create", (options?: Options) => Promise<unknown>>;

const refusal = { name: "NotAllowedError", message: "The operation either timed out or was not allowed." };

/** One frame's document: the preload runs in each by itself, and each refusal comes back to main. */
function frame(): { credentials: Credentials; real: { name: string; options: unknown }[] } {
  const real: { name: string; options: unknown }[] = [];
  const credentials: Credentials = {
    get: async (options) => { real.push({ name: "get", options }); return "chromium's answer"; },
    create: async (options) => { real.push({ name: "create", options }); return "chromium's answer"; },
  };
  Object.defineProperty(globalThis, "navigator", { configurable: true, value: { credentials } });
  (script.func as (...args: unknown[]) => void)(...script.args);
  return { credentials, real };
}

test("a passkey request the user would see fails at once in any frame, and says so to main each time", async () => {
  fake.sent.length = 0;
  for (const name of ["get", "create"] as const) {
    for (const each of [frame(), frame()]) {
      await assert.rejects(each.credentials[name]({ publicKey: {} }), refusal);
      assert.deepEqual(each.real, [], "Stock Electron would never answer it");
    }
  }
  assert.deepEqual(fake.sent, Array(4).fill("brigadier:guest-passkey-refused"));
});

test("passkey autofill and passwords stay Chromium's", async () => {
  fake.sent.length = 0;
  const { credentials, real } = frame();
  assert.equal(await credentials.get({ publicKey: {}, mediation: "conditional" }), "chromium's answer");
  assert.equal(await credentials.get({ password: true }), "chromium's answer");
  assert.equal(real.length, 2);
  assert.deepEqual(fake.sent, []);
});

test("a frame with no credentials API is left alone", () => {
  Object.defineProperty(globalThis, "navigator", { configurable: true, value: {} });
  assert.doesNotThrow(() => (script.func as (...args: unknown[]) => void)(...script.args));
});
