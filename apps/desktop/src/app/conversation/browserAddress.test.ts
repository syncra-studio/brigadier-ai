import assert from "node:assert/strict";
import { test } from "node:test";
import { browserAddress, webAddress } from "./browserAddress";

test("address entry opens web URLs and local hosts directly", () => {
  for (const [typed, expected] of [
    ["https://example.com/docs?q=hello", "https://example.com/docs?q=hello"],
    [" example.com/path ", "https://example.com/path"],
    ["localhost:3000", "http://localhost:3000"],
    ["127.0.0.1:1420/test", "http://127.0.0.1:1420/test"],
    ["[::1]:8080", "http://[::1]:8080"],
    ["0.0.0.0:8080", "http://0.0.0.0:8080"],
  ]) assert.equal(browserAddress(typed!), expected);
});

test("only address entry searches; blocked-page validation stays unchanged", () => {
  for (const query of ["hello world", "Brigadier", "a & b?", "日本語", "file:///private/notes", "javascript://alert(1)"])
    assert.equal(browserAddress(query), `https://www.google.com/search?q=${encodeURIComponent(query)}`);
  assert.equal(browserAddress("  "), null);
  assert.equal(webAddress("hello world"), null);
  assert.equal(webAddress("file:///private/notes"), null);
});
