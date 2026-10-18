import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

type Seen = {
  first: string | null;
  citations: string[];
  sources: { summary: string | null; open: boolean }[];
  linksShown: number;
  details: (string | null)[];
  report: (string | null)[];
  listed: (string | null)[];
  opened: { tab: { pageId: string | null; active: boolean } | null; dialog: string | null; read: string[] };
};

const PAGE = "5".repeat(32);
const see = async (t: Parameters<typeof renderFixtureLive>[0], query: string) =>
  JSON.parse(await renderFixtureLive(t, `research-answer.html?drive=1&${query}`, "research-answer-result")) as Seen;

// Q79: the recommendation first, numbered markers linked to their source, one folded
// "Sources · N" at the end, the full report one click away, and no Details when no code changed.
test("a research answer in a Chat: markers, folded sources, the report over the thread, no Details", { timeout: 90000 }, async (t) => {
  const seen = await see(t, "kind=chat");
  assert.match(seen.first ?? "", /^Start with Meshy's free plan/);
  assert.deepEqual(seen.citations, [
    "1 https://meshy.ai/pricing",
    "2 https://zoo.dev/text-to-cad",
    "3 https://hyper3d.ai/",
    "3 https://hyper3d.ai/",
    "2 https://zoo.dev/text-to-cad",
  ]);
  assert.deepEqual(seen.sources, [{ summary: "Sources · 3", open: false }]);
  assert.equal(seen.linksShown, 0);
  assert.deepEqual(seen.listed, ["Meshy pricing", "Zoo Text-to-CAD", "Rodin Gen-2.5"]);
  assert.deepEqual(seen.details, []);
  assert.deepEqual(seen.report, ["Full report"]);
  // A Chat has no tabs: the report opens over the thread.
  assert.equal(seen.opened.tab, null);
  assert.match(seen.opened.dialog ?? "", /AI models for 3D printing.*Meshy, Tripo and Rodin compared in full/);
  assert.deepEqual(seen.opened.read, [PAGE]);
});

test("a research answer in a session opens its report as a page tab and drops Details when no code changed", { timeout: 90000 }, async (t) => {
  const seen = await see(t, "kind=session");
  assert.deepEqual(seen.sources, [{ summary: "Sources · 3", open: false }]);
  assert.equal(seen.citations.length, 5);
  assert.deepEqual(seen.details, []);
  assert.deepEqual(seen.opened.tab, { pageId: PAGE, active: true });
  assert.equal(seen.opened.dialog, null);
});

test("a session answer whose request changed code keeps its folded Details beside its sources", { timeout: 90000 }, async (t) => {
  const seen = await see(t, "kind=session&changed=1");
  assert.deepEqual(seen.sources, [{ summary: "Sources · 3", open: false }]);
  assert.deepEqual(seen.details, ["Details"]);
});
