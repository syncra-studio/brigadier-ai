import assert from "node:assert/strict";
import { test } from "node:test";
import { browserAddress, shownAddress, siteSecurity, webAddress } from "./browserAddress";

test("address entry opens web URLs and local hosts directly", () => {
  for (const [typed, expected] of [
    ["https://example.com/docs?q=hello", "https://example.com/docs?q=hello"],
    [" example.com/path ", "https://example.com/path"],
    ["localhost:3000", "http://localhost:3000"],
    ["127.0.0.1:1420/test", "http://127.0.0.1:1420/test"],
    ["[::1]:8080", "http://[::1]:8080"],
    ["0.0.0.0:8080", "http://0.0.0.0:8080"],
    ["devbox:3000", "http://devbox:3000"],
    ["devbox:3000/api?x=1", "http://devbox:3000/api?x=1"],
    ["myserver/path", "http://myserver/path"],
    ["my-server/", "http://my-server/"],
  ]) assert.equal(browserAddress(typed!), expected);
});

test("only address entry searches; blocked-page validation stays unchanged", () => {
  for (const query of ["hello world", "Brigadier", "weather", "devbox?", "javascript:alert(1)", "-bad/path", "a & b?", "日本語", "file:///private/notes", "javascript://alert(1)"])
    assert.equal(browserAddress(query), `https://www.google.com/search?q=${encodeURIComponent(query)}`);
  assert.equal(browserAddress("  "), null);
  assert.equal(webAddress("hello world"), null);
  assert.equal(webAddress("file:///private/notes"), null);
});

test("the idle address drops https://, www. and a bare trailing slash; Show full URL keeps the rest", () => {
  const url = "https://www.example.com:8443/a/b?q=1#top";
  assert.equal(shownAddress(url, false), "example.com:8443");
  assert.equal(shownAddress(url, true), "example.com:8443/a/b?q=1#top");
  assert.equal(shownAddress("https://example.com/", false), "example.com");
  assert.equal(shownAddress("https://example.com/", true), "example.com");
  assert.equal(shownAddress("https://example.com/docs/", true), "example.com/docs/");
  assert.equal(shownAddress("https://example.com/?q=1", true), "example.com/?q=1");
  assert.equal(shownAddress("http://localhost:5173/checkout", false), "localhost:5173");
  assert.equal(shownAddress("http://localhost:5173/checkout", true), "localhost:5173/checkout");
  assert.equal(shownAddress("", false), "");
  assert.equal(shownAddress("about:blank", false), "about:blank");
  assert.equal(shownAddress("about:blank", true), "about:blank");
});

test("site information: https is secure, http is not unless it is on this computer", () => {
  assert.equal(siteSecurity("https://example.com/"), "secure");
  assert.equal(siteSecurity("http://example.com/"), "insecure");
  for (const local of ["http://localhost:3000/", "http://127.0.0.1:1420/", "http://[::1]:8080/", "http://0.0.0.0/", "http://app.localhost/"])
    assert.equal(siteSecurity(local), "local", local);
  assert.equal(siteSecurity(""), null);
  assert.equal(siteSecurity("about:blank"), null);
});
