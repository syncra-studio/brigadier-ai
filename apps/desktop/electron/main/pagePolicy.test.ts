import assert from "node:assert/strict";
import test from "node:test";

import { PAGE_CSP, contentType, pageFile } from "./pagePolicy.ts";

test("the page's origin serves its files and nothing outside them", () => {
  assert.equal(pageFile("/app/page", "app://brigadier/"), "/app/page/index.html");
  assert.equal(pageFile("/app/page", "app://brigadier/assets/a.js?x=1"), "/app/page/assets/a.js");
  // The URL parser settles dots itself, inside the root; an encoded slash reaches the check.
  assert.equal(pageFile("/app/page", "app://brigadier/%2e%2e/secret"), "/app/page/secret");
  assert.equal(pageFile("/app/page", "app://brigadier/..%2f..%2fsecret"), null);
  assert.equal(pageFile("/app/page", "app://other/index.html"), null);
  assert.equal(pageFile("/app/page", "file:///etc/passwd"), null);
  assert.equal(pageFile("/app/page", "app://brigadier/%zz"), null);
});

test("the policy has no way out for scripts or connections", () => {
  assert.match(PAGE_CSP, /script-src 'self' 'wasm-unsafe-eval'/);
  assert.doesNotMatch(PAGE_CSP, /script-src[^;]*'unsafe-inline'|http:|\*/);
  assert.equal(contentType("/a/b.WASM"), "application/wasm");
});
