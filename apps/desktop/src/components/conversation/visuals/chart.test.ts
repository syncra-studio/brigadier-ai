import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createServer, type ViteDevServer } from "vite";
import type * as Chart from "./chart";

import { comparison, single } from "@/fixtures/chartData";
import { renderFixturePage } from "@/fixtures/headless";

// As in render.test.ts: Vite loads JSX while node:test runs every assertion.
let server: ViteDevServer;
let readChart: typeof Chart.readChart;
let render: (value: unknown) => string;
before(async () => {
  server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom" });
  const chart = await server.ssrLoadModule("/src/components/conversation/visuals/chart.tsx");
  readChart = chart.readChart;
  render = (value) => renderToStaticMarkup(createElement(chart.VisualChart, { value }));
});
after(async () => { await server?.close(); });

for (const kind of ["bar", "line", "pie"]) {
  test(`readChart keeps the original ${kind} single-series format`, () => {
    assert.deepEqual(readChart({ ...single, kind }), { ...single, kind, series: null, data: [{ label: "Train", values: [35] }] });
  });
}
for (const kind of ["bar", "line"]) {
  test(`readChart reads named ${kind} series in declared order`, () => {
    const chart = readChart({ ...comparison, kind, series: ["SQLite", "Markdown"] });
    assert.deepEqual(chart?.series, ["SQLite", "Markdown"]);
    assert.deepEqual(chart?.data[0], { label: "Simplicity", values: [3, 5] });
  });
}
test("readChart treats series names containing dots and brackets as literal names", () => {
  assert.deepEqual(readChart({ ...comparison, series: ["db.v2[0]"], data: [{ label: "Search", values: { "db.v2[0]": -2 } }] })?.data[0]?.values, [-2]);
});
test("readChart rejects incomplete, mixed, nonfinite and excessive series", () => {
  for (const value of [
    { ...comparison, kind: "pie" },
    ...[null, [], [""], ["   "], ["x".repeat(201)], ["Markdown", "Markdown"], ["a", "b", "c", "d", "e", "f"], [1]].map((series) => ({ ...comparison, series })),
    ...[
      { label: "Search", values: { Markdown: 3 } },
      { label: "Search", values: { Markdown: 3, SQLite: 5, Extra: 2 } },
      { label: "Search", values: { Markdown: NaN, SQLite: 5 } },
      { label: "Search", values: { Markdown: 3, SQLite: Infinity } },
      { label: "Search", values: { Markdown: "3", SQLite: 5 } },
      { label: "Search", values: [3, 5] },
      { label: "Search", value: 3 },
      { label: "Search", value: 3, values: { Markdown: 3, SQLite: 5 } },
      { label: 3, values: { Markdown: 3, SQLite: 5 } },
    ].map((point) => ({ ...comparison, data: [point] })),
    { ...single, data: comparison.data },
  ]) assert.equal(readChart(value), null);
});
test("readChart retains the single-series bounds and nonnegative pie validation", () => {
  for (const value of [null, [], { ...single, kind: "area" }, { ...single, title: "x".repeat(301) },
    { ...single, description: "x".repeat(2001) }, { ...single, data: [] },
    { ...single, data: Array(201).fill(single.data[0]) },
    ...[NaN, Infinity, "5"].map((pointValue) => ({ ...single, data: [{ label: "Train", value: pointValue }] })),
    { ...single, data: [{ label: "x".repeat(201), value: 1 }] },
    { ...single, kind: "pie", data: [{ label: "Train", value: -1 }] },
  ]) assert.equal(readChart(value), null);
  assert.notEqual(readChart({ ...single, data: [{ label: "Train", value: -1 }] }), null);
});

test("VisualChart renders bar and line comparisons with distinct colours, legends and every criterion", { timeout: 60000 }, async (t) => {
  // Recharts fills its SVG after effects run, so check the actual plots in the existing browser fixture runner.
  const seen = JSON.parse(await renderFixturePage(t, "chart.html", "chart-result", 5000)) as Record<string, {
    marks: number; colors: string[]; legend: string[]; labels: string[]; description: string; scrolls: boolean; minWidth: string;
  }>;
  for (const kind of ["bar", "line"]) {
    const plot = seen[kind]!;
    assert.equal(plot.marks, kind === "bar" ? 8 : 2, kind);
    assert.deepEqual(plot.colors, ["var(--chart-1)", "var(--chart-2)"], kind);
    assert.deepEqual(plot.legend.toSorted(), comparison.series.toSorted(), kind);
    assert.deepEqual(plot.labels, comparison.data.map((point) => point.label), kind);
    assert.match(plot.description, /Simplicity: Markdown: 5, SQLite: 3/, kind);
    assert.equal(plot.scrolls, true, kind);
    assert.notEqual(plot.minWidth, "", kind);
  }
});
for (const kind of ["bar", "line", "pie"]) {
  test(`VisualChart renders legacy ${kind} data with its accessible values`, () => {
    const html = render({ ...single, kind });
    assert.match(html, /Train: 35/);
    assert.doesNotMatch(html, /recharts-legend/);
    assert.match(html, /Minutes; illustrative/);
  });
}
test("VisualChart shows the existing error for malformed comparisons", () => {
  assert.match(render({ ...comparison, series: [] }), /role="alert"/);
});
