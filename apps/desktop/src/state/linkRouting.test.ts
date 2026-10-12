import assert from "node:assert/strict";
import { test } from "node:test";

import { type LinkChoices, linkDestination, localUrl } from "./linkRouting";

const DEFAULTS: LinkChoices = { browserEnabled: true, webLinksIn: "system", localLinksIn: "brigadier" };
const IN_SESSION = { session: true, embedded: true };

test("local development URLs are loopback hosts and *.localhost only", () => {
  for (const url of ["http://localhost:5173/", "http://127.0.0.1:8080/x", "http://0.0.0.0:3000", "http://[::1]:4000/", "http://app.localhost/"])
    assert.ok(localUrl(url), url);
  for (const url of ["https://example.com/", "http://localhost.example.com/", "http://10.0.0.2/", "http://devbox:3000/", "not a url"])
    assert.ok(!localUrl(url), url);
});

test("by default web links go to the default browser and local dev URLs to a Browser tab", () => {
  assert.equal(linkDestination("https://example.com/", DEFAULTS, IN_SESSION), "system");
  assert.equal(linkDestination("http://localhost:5173/", DEFAULTS, IN_SESSION), "tab");
});

test("each kind of link follows its own choice, so everything can go to either", () => {
  const everythingInside: LinkChoices = { ...DEFAULTS, webLinksIn: "brigadier" };
  assert.equal(linkDestination("https://example.com/", everythingInside, IN_SESSION), "tab");
  const everythingOutside: LinkChoices = { ...DEFAULTS, localLinksIn: "system" };
  assert.equal(linkDestination("http://localhost:5173/", everythingOutside, IN_SESSION), "system");
});

test("off, every link goes to the default browser", () => {
  const off: LinkChoices = { browserEnabled: false, webLinksIn: "brigadier", localLinksIn: "brigadier" };
  for (const url of ["https://example.com/", "http://localhost:5173/"])
    assert.equal(linkDestination(url, off, IN_SESSION), "system", url);
});

test("a tab needs a session in front and a system that shows pages in the app", () => {
  assert.equal(linkDestination("http://localhost:5173/", DEFAULTS, { session: false, embedded: true }), "system");
  assert.equal(linkDestination("http://localhost:5173/", DEFAULTS, { session: true, embedded: false }), "system");
});

test("anything that isn't a web page opens nowhere", () => {
  for (const url of ["mailto:a@b.c", "file:///etc/passwd", "javascript:alert(1)", "brigadier://x"])
    assert.equal(linkDestination(url, DEFAULTS, IN_SESSION), null, url);
});
