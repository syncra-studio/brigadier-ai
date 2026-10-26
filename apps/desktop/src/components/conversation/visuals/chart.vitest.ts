import { act, cloneElement, createElement } from "react";
import { createRoot } from "react-dom/client";
import { describe, expect, it, vi } from "vitest";
import type * as Recharts from "recharts";

import { readChart, VisualChart } from "./chart";

// Supply dimensions without a browser layout engine, keeping the real Recharts plots.
vi.mock("recharts", async (original) => {
  const actual = await original<typeof Recharts>();
  return { ...actual, ResponsiveContainer: ({ children }: { children: React.ReactElement }) => cloneElement(children, { width: 1000, height: 256 } as object) };
});

const comparison = {
  kind: "bar", title: "Knowledge base options", description: "Scores 1–5; illustrative",
  series: ["Markdown", "SQLite"],
  data: [
    { label: "Simplicity", values: { Markdown: 5, SQLite: 3 } },
    { label: "Search", values: { Markdown: 3, SQLite: 5 } },
    { label: "Sync safety", values: { Markdown: 4, SQLite: 2 } },
    { label: "Portability", values: { Markdown: 5, SQLite: 4 } },
  ],
};
const single = { kind: "bar", title: "Travel time", description: "Minutes; illustrative", data: [{ label: "Train", value: 35 }] };
Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
async function render(value: unknown) {
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  try {
    await act(async () => { root.render(createElement(VisualChart, { value })); });
    return container.innerHTML;
  } finally {
    await act(async () => { root.unmount(); });
    container.remove();
  }
}

describe("readChart", () => {
  it.each(["bar", "line", "pie"])("keeps the original %s single-series format", (kind) => {
    expect(readChart({ ...single, kind })).toEqual({ ...single, kind, series: null, data: [{ label: "Train", values: [35] }] });
  });
  it.each(["bar", "line"])("reads named %s series in declared order", (kind) => {
    const chart = readChart({ ...comparison, kind, series: ["SQLite", "Markdown"] });
    expect(chart?.series).toEqual(["SQLite", "Markdown"]);
    expect(chart?.data[0]).toEqual({ label: "Simplicity", values: [3, 5] });
  });
  it("treats series names containing dots and brackets as literal names", () => {
    expect(readChart({ ...comparison, series: ["db.v2[0]"], data: [{ label: "Search", values: { "db.v2[0]": -2 } }] })?.data[0]?.values).toEqual([-2]);
  });
  it("rejects incomplete, mixed, nonfinite and excessive series", () => {
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
    ]) expect(readChart(value)).toBeNull();
  });
  it("retains the single-series bounds and nonnegative pie validation", () => {
    for (const value of [null, [], { ...single, kind: "area" }, { ...single, title: "x".repeat(301) },
      { ...single, description: "x".repeat(2001) }, { ...single, data: [] },
      { ...single, data: Array(201).fill(single.data[0]) },
      ...[NaN, Infinity, "5"].map((pointValue) => ({ ...single, data: [{ label: "Train", value: pointValue }] })),
      { ...single, data: [{ label: "x".repeat(201), value: 1 }] },
      { ...single, kind: "pie", data: [{ label: "Train", value: -1 }] },
    ]) expect(readChart(value)).toBeNull();
    expect(readChart({ ...single, data: [{ label: "Train", value: -1 }] })).not.toBeNull();
  });
});

describe("VisualChart", () => {
  it.each(["bar", "line"])("renders %s comparisons with distinct colours, a legend and every criterion", async (kind) => {
    const html = await render({ ...comparison, kind });
    expect(html).toContain("recharts-legend");
    expect(html).toContain("var(--chart-1)");
    expect(html).toContain("var(--chart-2)");
    const plot = document.createElement("div");
    plot.innerHTML = html;
    expect(plot.querySelectorAll(kind === "bar" ? ".recharts-bar-rectangle" : ".recharts-line-curve")).toHaveLength(kind === "bar" ? 8 : 2);
    expect(plot.querySelectorAll(".recharts-legend-item")).toHaveLength(2);
    for (const point of comparison.data) expect(html).toContain(`>${point.label}</tspan>`);
    expect(html).toContain("Markdown");
    expect(html).toContain("SQLite");
    expect(html).toContain("Simplicity: Markdown: 5, SQLite: 3");
    expect(html).toContain("overflow-x-auto");
    expect(html).toContain("min-width:");
  });
  it.each(["bar", "line", "pie"])("renders legacy %s data with its accessible values", async (kind) => {
    const html = await render({ ...single, kind });
    expect(html).toContain("Train: 35");
    expect(html).not.toContain("recharts-legend");
    expect(html).toContain("Minutes; illustrative");
  });
  it("shows the existing error for malformed comparisons", async () => {
    expect(await render({ ...comparison, series: [] })).toContain('role="alert"');
  });
});
