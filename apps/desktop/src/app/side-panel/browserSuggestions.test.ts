import assert from "node:assert/strict";
import { test } from "node:test";

import { addressSuggestions, stepSuggestion } from "./browserSuggestions";

const visit = (id: number, url: string, title = "") => ({ id, url, title, atMs: id, agent: false });
const tab = (id: string, url: string, title = "") => ({ id, url, title });

test("words first search the web, then the matching tabs to switch to, then the history", () => {
  const found = addressSuggestions(
    " docs ",
    [tab("self", "https://docs.test/a", "Docs"), tab("t1", "https://docs.test/b", "B"), tab("t2", "https://other.test/", "Other"), tab("t3", "about:blank", "docs")],
    [visit(3, "https://docs.test/b"), visit(2, "https://site.test/", "Read the docs"), visit(1, "https://site.test/", "Read the docs"), visit(0, "file:///docs")],
    "self",
  );
  assert.deepEqual(found, [
    { kind: "go", url: "https://www.google.com/search?q=docs", search: true, text: "docs" },
    { kind: "tab", id: "t1", url: "https://docs.test/b", title: "B" },
    // The open tab's page and a repeated visit show once; only web pages.
    { kind: "visit", id: 2, url: "https://site.test/", title: "Read the docs" },
  ]);
});

test("an address goes there, and its own open tab still offers a switch", () => {
  const found = addressSuggestions("docs.test/b", [tab("t1", "https://docs.test/b", "B")], [visit(1, "https://docs.test/b")], "self");
  assert.deepEqual(found, [
    { kind: "go", url: "https://docs.test/b", search: false, text: "docs.test/b" },
    { kind: "tab", id: "t1", url: "https://docs.test/b", title: "B" },
  ]);
});

test("nothing typed suggests nothing, and the rows are capped", () => {
  assert.deepEqual(addressSuggestions("  ", [tab("t1", "https://a.test/")], [], "self"), []);
  const tabs = Array.from({ length: 5 }, (_, n) => tab(`t${n}`, `https://a.test/${n}`));
  const visits = Array.from({ length: 20 }, (_, n) => visit(n, `https://a.test/v${n}`));
  const found = addressSuggestions("a.test", tabs, visits, "self");
  assert.equal(found.filter((row) => row.kind === "tab").length, 3);
  assert.equal(found.length, 8);
});

test("the arrows go round the rows through the field", () => {
  assert.equal(stepSuggestion(-1, 1, 3), 0);
  assert.equal(stepSuggestion(2, 1, 3), -1, "past the last, back in the field");
  assert.equal(stepSuggestion(-1, -1, 3), 2, "up from the field, the last row");
  assert.equal(stepSuggestion(0, -1, 3), -1);
  assert.equal(stepSuggestion(-1, 1, 0), -1);
});
