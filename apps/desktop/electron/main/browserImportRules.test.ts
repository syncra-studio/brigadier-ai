import assert from "node:assert/strict";
import { test } from "node:test";

import { cookieDetails, distinctDomains, firstImport, type ImportedRecord, importOrigins, isGoogleCookie, mergedRecord, passwordsStay, type ReadCookie, SESSION_COOKIE_SECONDS } from "./browserImportRules.ts";

const cookie = (fields: Partial<ReadCookie> = {}): ReadCookie => ({
  name: "sid",
  value: "v",
  domain: "app.example",
  host_only: true,
  path: "/",
  expires_unix: 2_000_000_000,
  secure: false,
  http_only: true,
  same_site: "Lax",
  source_insecure: true,
  ...fields,
});

test("a host-only cookie stays host-only; a domain cookie keeps its domain", () => {
  assert.equal(cookieDetails(cookie(), 0).domain, undefined);
  assert.equal(cookieDetails(cookie({ domain: ".example.com", host_only: false }), 0).domain, ".example.com");
  assert.equal(cookieDetails(cookie({ domain: ".example.com", host_only: false }), 0).url, "http://example.com/");
});

test("a __Host- cookie goes back as it was: host-only, secure, at /", () => {
  const details = cookieDetails(cookie({ name: "__Host-sid", secure: true, source_insecure: false }), 0);
  assert.equal(details.url, "https://app.example/");
  assert.equal(details.domain, undefined);
  assert.equal(details.secure, true);
  assert.equal(details.path, "/");
});

test("a session cookie gets its long expiry now; a dated one keeps its date", () => {
  assert.equal(cookieDetails(cookie({ expires_unix: null }), 100).expirationDate, 100 + SESSION_COOKIE_SECONDS);
  assert.equal(cookieDetails(cookie(), 100).expirationDate, 2_000_000_000);
});

test("the URL's scheme says who set the cookie", () => {
  assert.equal(cookieDetails(cookie({ source_insecure: true }), 0).url, "http://app.example/");
  assert.equal(cookieDetails(cookie({ source_insecure: false }), 0).url, "https://app.example/");
  assert.equal(cookieDetails(cookie({ secure: true, source_insecure: true }), 0).url, "https://app.example/");
});

test("sameSite follows the source, and none given is unspecified", () => {
  assert.equal(cookieDetails(cookie({ same_site: "None" }), 0).sameSite, "no_restriction");
  assert.equal(cookieDetails(cookie({ same_site: "Strict" }), 0).sameSite, "strict");
  assert.equal(cookieDetails(cookie({ same_site: null }), 0).sameSite, "unspecified");
});

test("only google.com and its subdomains count as Google's", () => {
  for (const domain of ["google.com", ".google.com", "accounts.google.com", ".ACCOUNTS.Google.com"]) assert.equal(isGoogleCookie(domain), true, domain);
  for (const domain of ["notgoogle.com", "google.com.evil.example", "google.co.uk", "youtube.com"]) assert.equal(isGoogleCookie(domain), false, domain);
});

test("distinct domains drop the dot and case, keep the first, and count them all", () => {
  assert.deepEqual(distinctDomains([".a.com", "A.com", "b.com", "c.com"], 2), { shown: ["a.com", "b.com"], count: 3 });
  assert.deepEqual(distinctDomains([""]), { shown: [], count: 0 });
});

test("the notice stays shown once", () => {
  assert.equal(firstImport({}), true);
  assert.equal(firstImport({ noticed: true }), false);
});

const record = (fields: Partial<ImportedRecord> = {}): ImportedRecord => ({
  atMs: 1,
  profiles: ["Personal"],
  imported: 3,
  skipped: 0,
  skippedDomains: [],
  skippedDomainCount: 0,
  appBoundDomains: [],
  history: 2,
  bookmarks: 1,
  passwords: 2,
  passwordsSkipped: 0,
  passwordsKept: false,
  googleLeftOut: 2,
  ...fields,
});

test("a browser's row names every profile imported and sums what they brought", () => {
  assert.equal(mergedRecord([]), null);
  const merged = mergedRecord([
    record({ skipped: 1, skippedDomains: ["a.com"], skippedDomainCount: 1, appBoundDomains: ["a.com"] }),
    record({ atMs: 5, profiles: ["Work"], imported: 2, skipped: 2, skippedDomains: ["a.com", "b.com"], skippedDomainCount: 30, passwordsKept: true, googleLeftOut: 0 }),
  ]);
  assert.deepEqual(merged, {
    atMs: 5,
    profiles: ["Personal", "Work"],
    imported: 5,
    skipped: 3,
    skippedDomains: ["a.com", "b.com"],
    skippedDomainCount: 30,
    appBoundDomains: ["a.com"],
    history: 4,
    bookmarks: 2,
    passwords: 4,
    passwordsSkipped: 0,
    passwordsKept: true,
    googleLeftOut: 2,
  });
});

test("a profile's origin is the browser it last had an import from", () => {
  assert.deepEqual(importOrigins({}), {});
  assert.deepEqual(
    importOrigins({
      "dia/Default": { into: "a", atMs: 1, browser: "Dia" },
      "firefox/x": { into: "a", atMs: 2, browser: "Firefox" },
      "dia/Profile 1": { into: "b", atMs: 3, browser: "Dia" },
      // An entry from before names were kept goes by its key.
      "firefox/old": { into: "c", atMs: 4 },
    }),
    { a: "Firefox", b: "Dia", c: "Firefox" },
  );
});

test("importing over a profile keeps its passwords only when the source was read and has none usable", () => {
  const web = { url: "https://a.com", password: "p" };
  const android = { url: "android://x", password: "p" };
  const blank = { url: "https://a.com", password: "" };
  const hasSite = (login: { url: string }) => login.url.startsWith("https:");
  assert.equal(passwordsStay(true, true, [], hasSite), true);
  assert.equal(passwordsStay(true, true, [android, blank], hasSite), true);
  assert.equal(passwordsStay(true, true, [android, web], hasSite), false);
  assert.equal(passwordsStay(true, false, [], hasSite), false, "a source that couldn't be read says so itself");
  assert.equal(passwordsStay(false, true, [], hasSite), false, "a new profile has nothing to keep");
});
