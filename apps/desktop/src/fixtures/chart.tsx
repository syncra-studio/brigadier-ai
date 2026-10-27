import { createRoot } from "react-dom/client";

import { VisualChart } from "@/components/conversation/visuals/chart";
import { comparison } from "@/fixtures/chartData";

// A narrow container exercises real ResponsiveContainer measurements and category scrolling.
createRoot(document.getElementById("root")!).render(
  <main className="w-80">
    {["bar", "line"].map((kind) => <section key={kind} data-kind={kind}><VisualChart value={{ ...comparison, kind }} /></section>)}
  </main>,
);

async function drive() {
  const seen: Record<string, unknown> = {};
  for (const kind of ["bar", "line"]) {
    const plot = () => document.querySelector(`[data-kind=${kind}]`);
    const marks = kind === "bar" ? ".recharts-bar-rectangle" : ".recharts-line-curve";
    const labels = ".recharts-xAxis-tick-labels .recharts-cartesian-axis-tick-value";
    const expected = kind === "bar" ? 8 : 2;
    for (let tries = 0; tries < 100 && (plot()?.querySelectorAll(marks).length !== expected ||
      plot()?.querySelectorAll(labels).length !== comparison.data.length ||
      plot()?.querySelectorAll(".recharts-legend-item-text").length !== comparison.series.length); tries++) {
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    const chart = plot()!;
    const scroller = chart.querySelector<HTMLElement>(".overflow-x-auto")!;
    const description = chart.querySelector<HTMLElement>('[role="img"]')!;
    seen[kind] = {
      marks: chart.querySelectorAll(marks).length,
      colors: [...new Set([...chart.querySelectorAll(kind === "bar" ? `${marks} path` : marks)]
        .map((mark) => mark.getAttribute(kind === "bar" ? "fill" : "stroke")))],
      legend: [...chart.querySelectorAll(".recharts-legend-item-text")].map((item) => item.textContent),
      labels: [...chart.querySelectorAll(labels)].map((tick) => tick.textContent),
      description: description.getAttribute("aria-label"),
      scrolls: scroller.scrollWidth > scroller.clientWidth,
      minWidth: description.style.minWidth,
    };
  }
  const result = document.createElement("pre");
  result.id = "chart-result";
  result.textContent = JSON.stringify(seen);
  document.body.append(result);
}
void drive();
