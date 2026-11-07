import assert from "node:assert/strict";
import test from "node:test";

import type { Cookie } from "electron";

import { persistentCopy } from "./sessionCookies.ts";

const base: Cookie = { name: "sid", value: "v", path: "/", secure: true, httpOnly: true, sameSite: "lax", session: true };
const NOW = 1_800_000_000;

test("a host-only session cookie stays host-only, and gains an expiry", () => {
  const copy = persistentCopy({ ...base, domain: "a.example", hostOnly: true }, NOW)!;
  assert.equal(copy.url, "https://a.example/");
  assert.equal("domain" in copy, false);
  assert.equal(copy.expirationDate, NOW + 400 * 24 * 60 * 60);
  assert.equal(copy.httpOnly, true);
  assert.equal(copy.sameSite, "lax");
});

test("a domain cookie keeps its domain, and its URL is a host in it", () => {
  const copy = persistentCopy({ ...base, domain: ".example.com", hostOnly: false, secure: false, path: "/app" }, NOW)!;
  assert.equal(copy.domain, ".example.com");
  assert.equal(copy.url, "http://example.com/app");
});

test("a persistent cookie isn't touched", () => {
  assert.equal(persistentCopy({ ...base, domain: "a.example", session: false }, NOW), null);
});
