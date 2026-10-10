import { useMemo, useState } from "react";

import { useSize } from "@/hooks/use-size";
import type { Forecast, QuotaSample } from "@/ipc/generated";
import { formatTime } from "@/lib/format";
import { usageTone } from "@/lib/quota";
import { formatResetAt } from "@/lib/routing";
import { tokenPx } from "@/lib/tokens";
import { cn } from "@/lib/utils";
import { useApp } from "@/state/store";

const TICKS = [0, 50, 100];

/**
 * Used % across a window's span, from its start to its reset: the samples as a line over a
 * faint wash, the rolling estimate dashed from now to the reset (or to where it runs out), and a
 * hairline at now. The line is coloured by how much is used, the estimate never. Hovering finds the nearest sample.
 */
export function WindowChart({
  samples,
  startMs,
  endMs,
  nowMs,
  usedPercent,
  forecast,
  label,
}: {
  samples: readonly QuotaSample[];
  startMs: number;
  endMs: number;
  nowMs: number;
  usedPercent: number;
  forecast: Forecast | null;
  label: string;
}) {
  const { ref, size } = useSize();
  const density = useApp((s) => s.settings.density);
  const [hover, setHover] = useState<number | null>(null);
  // The line and its mark at now take usage's colour by how much is used; the estimate stays neutral.
  const usage = usageTone(usedPercent);

  // Geometry comes from the spacing token, re-read when density switches it.
  // oxlint-disable-next-line react-hooks/exhaustive-deps
  const unit = useMemo(() => tokenPx("--spacing"), [density]);
  const padLeft = unit * 8;
  const padRight = unit * 2;
  const padTop = unit * 1.5;
  const padBottom = unit * 1.5;
  const plotWidth = Math.max(0, size.width - padLeft - padRight);
  const plotHeight = Math.max(0, size.height - padTop - padBottom);
  const span = Math.max(1, endMs - startMs);
  const now = Math.min(Math.max(nowMs, startMs), endMs);

  const x = (ms: number) => padLeft + ((Math.min(Math.max(ms, startMs), endMs) - startMs) / span) * plotWidth;
  const y = (percent: number) => padTop + plotHeight * (1 - Math.min(100, Math.max(0, percent)) / 100);

  // The samples in this span, ending at the value read now.
  const points = useMemo(() => {
    const inSpan = samples.filter((sample) => sample.atMs >= startMs && sample.atMs <= now);
    const last = inSpan.at(-1);
    if (!last || last.atMs < now) inSpan.push({ atMs: now, usedPercent });
    return inSpan;
  }, [samples, startMs, now, usedPercent]);

  const line = points
    .map((point, index) => `${index === 0 ? "M" : "L"}${x(point.atMs)},${y(point.usedPercent)}`)
    .join("");
  const first = points[0];
  const area =
    first && points.length > 1
      ? `${line}L${x(now)},${y(0)}L${x(first.atMs)},${y(0)}Z`
      : null;

  // Where the estimate ends: the reset, or the moment it runs out before it.
  const projection =
    forecast && now < endMs
      ? forecast.runsOutAtMs !== null && forecast.runsOutAtMs < endMs
        ? { atMs: forecast.runsOutAtMs, percent: 100, runsOut: true }
        : { atMs: endMs, percent: forecast.projectedAtReset, runsOut: false }
      : null;

  const hovered = hover === null ? null : points[hover];
  const description = `${label}: ${Math.round(usedPercent)}% used, resets ${formatResetAt(endMs, nowMs)}${
    projection ? `, about ${Math.round(Math.min(projection.percent, 999))}% by reset at the recent rate` : ""
  }`;

  return (
    <div
      ref={ref}
      className="relative h-16 w-full"
      onPointerLeave={() => setHover(null)}
      onPointerMove={(event) => {
        if (plotWidth === 0 || points.length === 0) return;
        const bounds = event.currentTarget.getBoundingClientRect();
        const at = startMs + ((event.clientX - bounds.left - padLeft) / plotWidth) * span;
        let nearest = 0;
        points.forEach((point, index) => {
          if (Math.abs(point.atMs - at) < Math.abs((points[nearest]?.atMs ?? 0) - at)) nearest = index;
        });
        setHover(at >= startMs - span * 0.02 && at <= now + span * 0.02 ? nearest : null);
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
              <line
                x1={padLeft}
                x2={padLeft + plotWidth}
                y1={y(tick)}
                y2={y(tick)}
                className="stroke-border"
              />
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
          {area && <path d={area} className="fill-chart-2/10" />}
          <path
            d={line}
            className={cn(usage.stroke, "fill-none stroke-2")}
            strokeLinejoin="round"
            strokeLinecap="round"
          />
          {projection && (
            <>
              <line
                x1={x(now)}
                y1={y(usedPercent)}
                x2={x(projection.atMs)}
                y2={y(projection.percent)}
                strokeDasharray={`${unit} ${unit}`}
                className="stroke-muted-foreground/60 stroke-2"
                strokeLinecap="round"
              />
              {projection.runsOut && (
                <circle
                  cx={x(projection.atMs)}
                  cy={y(100)}
                  r={unit}
                  className="fill-muted-foreground stroke-card stroke-2"
                />
              )}
            </>
          )}
          <line
            x1={x(now)}
            x2={x(now)}
            y1={padTop}
            y2={padTop + plotHeight}
            className="stroke-foreground/30 stroke-1"
          />
          <circle
            cx={x(now)}
            cy={y(usedPercent)}
            r={unit}
            className={cn(usage.mark, "stroke-card stroke-2")}
          />
          {hovered && (
            <line
              x1={x(hovered.atMs)}
              x2={x(hovered.atMs)}
              y1={padTop}
              y2={padTop + plotHeight}
              className="stroke-foreground/50 stroke-1"
            />
          )}
        </svg>
      )}
      {hovered && hover !== null && (
        <div
          role="status"
          className="bg-popover text-popover-foreground rounded-surface pointer-events-none absolute -top-1 z-10 flex -translate-y-full flex-col border px-2 py-1 text-xs whitespace-nowrap"
          style={
            x(hovered.atMs) > size.width / 2
              ? { right: `${size.width - x(hovered.atMs)}px` }
              : { left: `${x(hovered.atMs)}px` }
          }
        >
          <span className="font-medium tabular-nums">{Math.round(hovered.usedPercent)}% used</span>
          <span className="text-muted-foreground">{formatTime(hovered.atMs)}</span>
        </div>
      )}
    </div>
  );
}
