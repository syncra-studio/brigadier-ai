import assert from "node:assert/strict";
import { test } from "node:test";
import { faviconFits } from "./browsers";

test("favicons follow same-document navigations but not other pages", () => {
  const loaded = { url: "https://example.com/app?x=1", loading: false };
  const loading = { ...loaded, loading: true };
  assert.equal(faviconFits(loaded, "https://example.com/app?x=1"), true);
  assert.equal(faviconFits(loading, "https://example.com/app?x=1#section"), true);
  assert.equal(faviconFits(loaded, "https://example.com/app/inbox"), true);
  assert.equal(faviconFits(loading, "https://example.com/previous"), false);
  assert.equal(faviconFits(loaded, "https://other.example/app?x=1"), false);
  assert.equal(faviconFits(loaded, "not a url"), false);
  assert.equal(faviconFits(undefined, "https://example.com/app?x=1"), false);
});
