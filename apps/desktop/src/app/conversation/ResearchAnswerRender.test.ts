import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

type Seen = {
  first: string | null;
  links: string[];
  folds: (string | null)[];
  markers: number;
  report: (string | null)[];
  bold: (string | null)[];
  items: (string | null)[];
  preview: string | null;
  browsed: string | null;
  system: string[];
  opened: { tab: { pageId: string | null; active: boolean } | null; dialog: string | null; read: string[] };
};

const PAGE = "5".repeat(32);
const see = async (t: Parameters<typeof renderFixtureLive>[0], query: string) =>
  JSON.parse(await renderFixtureLive(t, `research-answer.html?drive=1&${query}`, "research-answer-result")) as Seen;

const LINKS = [
  "pricing https://meshy.ai/pricing",
  "Zoo https://zoo.dev/text-to-cad",
  "Rodin https://hyper3d.ai/",
];

// Q87: the outcome first, each source linked where it is used, no Sources fold or markers, the
// full report one click away, and no Details when no code changed.
test("a research answer in a Chat: inline sources, no folds, the report over the thread", { timeout: 90000 }, async (t) => {
  const seen = await see(t, "kind=chat");
  assert.match(seen.first ?? "", /^Start with Meshy's free plan/);
  assert.deepEqual(seen.links, LINKS);
  assert.deepEqual(seen.folds, []);
  assert.equal(seen.markers, 0);
  assert.deepEqual(seen.report, ["Full report"]);
  // A Chat has no tabs: the report opens over the thread.
  assert.equal(seen.opened.tab, null);
  assert.match(seen.opened.dialog ?? "", /AI models for 3D printing.*Meshy, Tripo and Rodin compared in full/);
  assert.deepEqual(seen.opened.read, [PAGE]);
});

test("a research answer in a session opens its report as a page tab and drops Details when no code changed", { timeout: 90000 }, async (t) => {
  const seen = await see(t, "kind=session");
  assert.deepEqual(seen.links, LINKS);
  assert.deepEqual(seen.folds, []);
  assert.deepEqual(seen.opened.tab, { pageId: PAGE, active: true });
  assert.equal(seen.opened.dialog, null);
});

test("a session answer whose request changed code keeps Details, its one fold", { timeout: 90000 }, async (t) => {
  const seen = await see(t, "kind=session&changed=1");
  assert.deepEqual(seen.folds, ["Details"]);
  assert.deepEqual(seen.links, LINKS);
});

// Q87: a turn whose answer names exactly one local page left running ends with a web preview
// that opens it in the session's browser; two pages, or none, and there is no card.
test("an answer naming one local page gets a web preview that opens in the browser", { timeout: 90000 }, async (t) => {
  const one = await see(t, "kind=session&preview=1");
  assert.equal(one.preview, "Web previewlocalhost:5173Open");
  assert.equal(one.browsed, "http://localhost:5173/");
  const two = await see(t, "kind=session&preview=2");
  assert.equal(two.preview, null);
  const none = await see(t, "kind=session");
  assert.equal(none.preview, null);
});

// A late note under the answer leaves its preview; a click goes where Settings › Browser sends
// local pages.
test("a web preview stays under a late note and opens where local pages go", { timeout: 90000 }, async (t) => {
  const late = await see(t, "kind=session&preview=1&late=1");
  assert.equal(late.preview, "Web previewlocalhost:5173Open");
  assert.equal(late.browsed, "http://localhost:5173/");
  const system = await see(t, "kind=session&preview=1&local=system");
  assert.equal(system.browsed, null);
  assert.deepEqual(system.system, ["http://localhost:5173/"]);
});

// An answer written before Q87 reads as plain Markdown: its list of sources shows as written,
// nothing folds and no marker turns into a badge.
test("an older answer's numbered markers and Sources list read as written", { timeout: 90000 }, async (t) => {
  const seen = await see(t, "kind=chat&old=1");
  assert.deepEqual(seen.folds, []);
  assert.equal(seen.markers, 0);
  assert.match(seen.first ?? "", /\[1\]/);
  assert.deepEqual(seen.links, [
    "Meshy pricing https://meshy.ai/pricing",
    "Zoo Text-to-CAD https://zoo.dev/text-to-cad",
    "Rodin Gen-2.5 https://hyper3d.ai/",
  ]);
});

// #319: bold that opens a line or list item shows plain, whatever the model wrote; bold inside a
// sentence stays, and the words stay as written.
for (const [kind, query] of [
  ["a Chat's research answer", "kind=chat&bold=1"],
  ["a session's answer without Details", "kind=session&bold=1&details=0"],
] as const) {
  test(`${kind}: no line or item opens in bold, and mid-sentence bold stays`, { timeout: 90000 }, async (t) => {
    const seen = await see(t, query);
    assert.deepEqual(seen.bold, ["stops a failed print"]);
    assert.match(seen.items[0] ?? "", /^Figurines and gifts: Meshy, or Rodin/);
    assert.match(seen.items[1] ?? "", /^Zoo writes editable CAD/);
  });
}
