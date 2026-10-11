import { useMemo, useState } from "react";

import { useSize } from "@/hooks/use-size";
import type { QuotaSample } from "@/ipc/generated";
import { tokenPx } from "@/lib/tokens";
import { cn } from "@/lib/utils";
import { useApp } from "@/state/store";

export type ChartSeries = { id: string; title: string; samples: readonly QuotaSample[] };

/*
 * Each window's line keeps its colour in a fixed order; the third and fourth are dashed and
 * dotted too, so no two lines are told apart by colour alone.
 */
const STYLES = [
  { stroke: "stroke-chart-2", mark: "fill-chart-2", swatch: "border-chart-2", dash: undefined },
  { stroke: "stroke-chart-1", mark: "fill-chart-1", swatch: "border-chart-1", dash: undefined },
  { stroke: "stroke-chart-3", mark: "fill-chart-3", swatch: "border-chart-3 border-dashed", dash: "4 3" },
  { stroke: "stroke-foreground/60", mark: "fill-foreground/60", swatch: "border-foreground/60 border-dotted", dash: "1 3" },
] as const;

const TICKS = [0, 50, 100];
const X_TICKS = 4;

const tickDate = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" });
const hoverDate = new Intl.DateTimeFormat(undefined, {
  weekday: "short",
  month: "short",
  day: "numeric",
  hour: "numeric",
  minute: "2-digit",
});

/** What a series read at `atMs`: its last sample then, if any. */
export function valueAt(samples: readonly QuotaSample[], atMs: number): number | null {
  let value: number | null = null;
  for (const sample of samples) {
    if (sample.atMs > atMs) break;
    value = sample.usedPercent;
  }
  return value;
}

/**
 * How much of each window was used over a range, a line per window on one 0–100% scale, with
 * a legend over it. Hovering shows a crosshair and a card: the moment, then each window's
 * share used then.
 */
export function UsageChart({
  series,
  startMs,
  endMs,
}: {
  series: readonly ChartSeries[];
  startMs: number;
  endMs: number;
}) {
  const { ref, size } = useSize();
  const density = useApp((s) => s.settings.density);
  const [hoverMs, setHoverMs] = useState<number | null>(null);
  // Geometry comes from the spacing token, re-read when density switches it.
  // oxlint-disable-next-line react-hooks/exhaustive-deps
  const unit = useMemo(() => tokenPx("--spacing"), [density]);
  const padLeft = unit * 9;
  const padRight = unit * 2;
  const padTop = unit * 2;
  const padBottom = unit * 6;
  const plotWidth = Math.max(0, size.width - padLeft - padRight);
  const plotHeight = Math.max(0, size.height - padTop - padBottom);
  const span = Math.max(1, endMs - startMs);
  const x = (ms: number) => padLeft + ((Math.min(Math.max(ms, startMs), endMs) - startMs) / span) * plotWidth;
  const y = (percent: number) => padTop + plotHeight * (1 - Math.min(100, Math.max(0, percent)) / 100);

  const sorted = useMemo(
    () => series.map((entry) => ({ ...entry, samples: entry.samples.toSorted((a, b) => a.atMs - b.atMs) })),
    [series],
  );
  const shown = sorted.slice(0, STYLES.length);
  const lines = shown.map((entry) =>
    entry.samples
      .filter((sample) => sample.atMs >= startMs && sample.atMs <= endMs)
      .map((sample, index) => `${index === 0 ? "M" : "L"}${x(sample.atMs)},${y(sample.usedPercent)}`)
      .join(""),
  );
  const ticks = Array.from({ length: X_TICKS }, (_, index) => startMs + (span * index) / (X_TICKS - 1));
  const hovered =
    hoverMs === null
      ? null
      : shown.map((entry, index) => ({ entry, index, value: valueAt(entry.samples, hoverMs) }));
  const description = `Share used over the range: ${shown
    .map((entry) => {
      const now = valueAt(entry.samples, endMs);
      return `${entry.title} ${now === null ? "not read" : `${Math.round(now)}% now`}`;
    })
    .join(", ")}`;

  return (
    <div className="flex flex-col gap-2">
      {shown.length > 1 && (
        <ul aria-hidden className="text-muted-foreground flex flex-wrap gap-x-4 gap-y-1 text-xs">
          {shown.map((entry, index) => (
            <li key={entry.id} className="flex items-center gap-1.5">
              <Swatch index={index} />
              {entry.title}
            </li>
          ))}
        </ul>
      )}
      <div
        ref={ref}
        data-slot="usage-chart"
        className="relative h-36 w-full"
        onPointerLeave={() => setHoverMs(null)}
        onPointerMove={(event) => {
          if (plotWidth === 0) return;
          const bounds = event.currentTarget.getBoundingClientRect();
          const at = startMs + ((event.clientX - bounds.left - padLeft) / plotWidth) * span;
          setHoverMs(at >= startMs && at <= endMs ? at : null);
        }}
      >
        {size.width > 0 && (
          <svg
            role="img"
            aria-label={description}
            viewBox={`0 0 ${size.width} ${size.height}`}
            className="absolute inset-0 size-full overflow-visible"
          >
            {TICKS.map((tick) => (
              <g key={tick}>
                <line x1={padLeft} x2={padLeft + plotWidth} y1={y(tick)} y2={y(tick)} className="stroke-border" />
                <text
                  x={padLeft - unit}
                  y={y(tick)}
                  textAnchor="end"
                  dominantBaseline="middle"
                  className="fill-muted-foreground text-2xs tabular-nums"
                >
                  {tick}%
                </text>
              </g>
            ))}
            {ticks.map((at, index) => (
              <text
                key={at}
                x={x(at)}
                y={padTop + plotHeight + unit * 4}
                textAnchor={index === 0 ? "start" : index === X_TICKS - 1 ? "end" : "middle"}
                className="fill-muted-foreground text-2xs tabular-nums"
              >
                {tickDate.format(at)}
              </text>
            ))}
            {lines.map((line, index) => (
              <path
                key={shown[index]!.id}
                d={line}
                strokeDasharray={STYLES[index]!.dash}
                className={cn(STYLES[index]!.stroke, "fill-none stroke-2")}
                strokeLinejoin="round"
                strokeLinecap="round"
              />
            ))}
            {hoverMs !== null && (
              <>
                <line
                  x1={x(hoverMs)}
                  x2={x(hoverMs)}
                  y1={padTop}
                  y2={padTop + plotHeight}
                  className="stroke-foreground/40 stroke-1"
                />
                {hovered?.map(
                  ({ entry, index, value }) =>
                    value !== null && (
                      <circle
                        key={entry.id}
                        cx={x(hoverMs)}
                        cy={y(value)}
                        r={unit}
                        className={cn(STYLES[index]!.mark, "stroke-card stroke-2")}
                      />
                    ),
                )}
              </>
            )}
          </svg>
        )}
        {hoverMs !== null && hovered && (
          <div
            role="status"
            data-slot="usage-chart-tooltip"
            className="bg-popover text-popover-foreground rounded-surface border-divider pointer-events-none absolute top-0 z-10 flex min-w-40 flex-col gap-1 border px-3 py-2 text-xs shadow-md"
            style={
              x(hoverMs) > size.width / 2
                ? { right: `${size.width - x(hoverMs) + unit * 2}px` }
                : { left: `${x(hoverMs) + unit * 2}px` }
            }
          >
            <span className="font-medium">{hoverDate.format(hoverMs)}</span>
            {hovered.map(({ entry, index, value }) => (
              <span key={entry.id} className="flex items-center gap-1.5">
                <Swatch index={index} />
                <span className="text-muted-foreground min-w-0 flex-1 truncate">{entry.title}</span>
                <span className="tabular-nums">{value === null ? "—" : `${Math.round(value)}% used`}</span>
              </span>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

/** A series' mark in the legend and the hover card. */
function Swatch({ index }: { index: number }) {
  return <span aria-hidden className={cn("w-3 shrink-0 border-t-2", STYLES[index]!.swatch)} />;
}
