import assert from "node:assert/strict";
import { test } from "node:test";

import type { BrowserVisit } from "@/ipc/generated";
import { suggestedSites } from "./newTabSites";

const visit = (id: number, url: string, title = "", atMs = id): BrowserVisit => ({ id, url, title, atMs, agent: false });
test("suggested sites use newest history pages, deduplicate origins, and keep four", () => {
  const history = [visit(1, "https://one.test/old", "Old"), visit(8, "https://one.test/new", " New "),
    visit(6, "https://two.test/"), visit(5, "https://three.test/"), visit(4, "https://four.test/"), visit(3, "https://five.test/")];
  assert.deepEqual(suggestedSites(history), [
    { origin: "https://one.test", url: "https://one.test/new", title: "New" },
    { origin: "https://two.test", url: "https://two.test/", title: "two.test" },
    { origin: "https://three.test", url: "https://three.test/", title: "three.test" },
    { origin: "https://four.test", url: "https://four.test/", title: "four.test" },
  ]);
  assert.equal(history[0]?.id, 1, "does not reorder stored history");
});
test("empty, dismissed, malformed, non-web and credential-bearing history do not become tiles", () => {
  assert.deepEqual(suggestedSites([]), []);
  assert.deepEqual(suggestedSites([
    visit(1, "not a URL"), visit(2, "file:///private/notes"), visit(3, "javascript:alert(1)"),
    visit(4, "https://user:secret@site.test/"), visit(5, "https://site.test/", "Site"),
  ], ["https://site.test"]), []);
});
test("a titleless page uses the host and equal timestamps use the latest visit id", () => {
  assert.deepEqual(suggestedSites([visit(1, "https://www.site.test/old", "Old", 5), visit(2, "https://www.site.test/new", " ", 5)]),
    [{ origin: "https://www.site.test", url: "https://www.site.test/new", title: "site.test" }]);
});
