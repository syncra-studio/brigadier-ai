import assert from "node:assert/strict";
import { test } from "node:test";

import { webPreviewUrl } from "@/app/conversation/activity/webPreview";

test("a turn's answer gets a web preview when it names exactly one local address with a port", () => {
  assert.equal(webPreviewUrl("The app runs at http://localhost:3000."), "http://localhost:3000/");
  assert.equal(webPreviewUrl("Open [the app](http://127.0.0.1:5173/) or http://127.0.0.1:5173/ again."), "http://127.0.0.1:5173/");
  assert.equal(webPreviewUrl("See http://app.localhost:8080/todos"), "http://app.localhost:8080/todos");
  // Its query and fragment stay: they pick the page.
  assert.equal(webPreviewUrl("Open http://localhost:5173/#/todos to try it."), "http://localhost:5173/#/todos");
  assert.equal(webPreviewUrl("Try http://localhost:5173/?token=demo"), "http://localhost:5173/?token=demo");
  // None, a public site, no port, or two different pages: no card.
  assert.equal(webPreviewUrl("Done: the tests pass."), null);
  assert.equal(webPreviewUrl("Docs: https://example.com:8443/"), null);
  assert.equal(webPreviewUrl("http://localhost/ answers"), null);
  assert.equal(webPreviewUrl("The site is on http://localhost:3000 and the API on http://localhost:4000"), null);
});
