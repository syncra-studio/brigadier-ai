import { useMemo, useState } from "react";

import {
  type DerivedLog,
  formatTokens,
  GROUPS,
  KIND_LABELS,
  sumOf,
} from "@/app/inspector/orchestratorLog";
import { useSize } from "@/hooks/use-size";
import type { RebirthThresholds } from "@/ipc/generated";
import { tokenPx } from "@/lib/tokens";

/** A round upper bound for the y axis: 1, 2 or 5 times a power of ten. */
function niceMax(value: number): number {
  if (value <= 0) return 1;
  const power = 10 ** Math.floor(Math.log10(value));
  const step = [1, 2, 5, 10].find((factor) => factor * power >= value) ?? 10;
  return step * power;
}

/**
 * Cumulative estimated tokens Brigadier put into the orchestrator's context, one step per
 * injection, stacked by what it was; the CLI's own reported context size is drawn on the same
 * token axis. If the context grows only by messages and reports, the dots ride the stack.
 * The rebirth thresholds are drawn across it, and each rebirth is a marker where the new CLI
 * generation's stack starts afresh.
 */
export function ContextChart({
  log,
  thresholds,
}: {
  log: DerivedLog;
  thresholds: RebirthThresholds | null;
}) {
  const { ref, size } = useSize();
  const [hover, setHover] = useState<number | null>(null);

  // Geometry comes from the spacing token.
  const unit = useMemo(() => tokenPx("--spacing"), []);
  const n = log.injections.length;
  const padLeft = unit * 9;
  const padRight = unit * 2;
  const padTop = unit * 2;
  const padBottom = unit * 2;
  const plotWidth = Math.max(0, size.width - padLeft - padRight);
  const plotHeight = Math.max(0, size.height - padTop - padBottom);

  const maxValue = niceMax(
    Math.max(
      ...log.cumulative.map(sumOf),
      ...log.context.map((point) => point.usedTokens),
      thresholds?.swapTokens ?? 0,
    ),
  );
  const x = (step: number) => padLeft + (n === 0 ? 0 : (step / n) * plotWidth);
  const y = (value: number) => padTop + plotHeight - (value / maxValue) * plotHeight;

  const paths = useMemo(() => {
    if (n === 0 || plotWidth === 0) return [];
    return GROUPS.map((group, groupIndex) => {
      const below = GROUPS.slice(0, groupIndex);
      const upper: string[] = [];
      const lower: string[] = [];
      log.cumulative.forEach((totals, index) => {
        const base = below.reduce((sum, entry) => sum + totals[entry.id], 0);
        const top = base + totals[group.id];
        upper.push(`L${x(index)},${y(top)}L${x(index + 1)},${y(top)}`);
        lower.push(`L${x(index + 1)},${y(base)}L${x(index)},${y(base)}`);
      });
      const d = `M${x(0)},${y(0)}${upper.join("")}${lower.toReversed().join("")}Z`;
      return { id: group.id, fill: group.fill, d };
    });
    // x and y are pure functions of the values listed here.
    // oxlint-disable-next-line react-hooks/exhaustive-deps
  }, [log.cumulative, n, plotWidth, plotHeight, maxValue, padLeft, padTop]);

  const ticks = [0, maxValue / 2, maxValue];
  const hovered = hover === null ? null : log.injections[hover];
  const hoveredTotals = hover === null ? null : log.cumulative[hover];

  return (
    <div className="flex flex-col gap-2">
      <div
        ref={ref}
        className="relative h-36 w-full"
        onPointerLeave={() => setHover(null)}
        onPointerMove={(event) => {
          if (n === 0 || plotWidth === 0) return;
          const bounds = event.currentTarget.getBoundingClientRect();
          const position = (event.clientX - bounds.left - padLeft) / plotWidth;
          const index = Math.floor(position * n);
          setHover(index >= 0 && index < n ? index : null);
        }}
      >
        {size.width > 0 && (
          <svg
            role="img"
            aria-label={`Orchestrator context: ${formatTokens(sumOf(log.totals))} tokens injected over ${n} injections and ${log.generations.length} CLI generations`}
            viewBox={`0 0 ${size.width} ${size.height}`}
            className="absolute inset-0 size-full overflow-visible"
          >
            {ticks.map((tick) => (
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
                  {formatTokens(tick)}
                </text>
              </g>
            ))}
            {paths.map((path) => (
              <path
                key={path.id}
                d={path.d}
                className={`${path.fill} stroke-background stroke-1`}
                strokeLinejoin="round"
              />
            ))}
            {hover !== null && (
              <line
                x1={(x(hover) + x(hover + 1)) / 2}
                x2={(x(hover) + x(hover + 1)) / 2}
                y1={padTop}
                y2={padTop + plotHeight}
                className="stroke-foreground/40 stroke-1"
              />
            )}
            {thresholds &&
              [
                { id: "prepare", tokens: thresholds.prepareTokens, stroke: "stroke-warning", fill: "fill-warning" },
                { id: "swap", tokens: thresholds.swapTokens, stroke: "stroke-destructive", fill: "fill-destructive" },
              ].map((line) => (
                <g key={line.id}>
                  <line
                    x1={padLeft}
                    x2={padLeft + plotWidth}
                    y1={y(line.tokens)}
                    y2={y(line.tokens)}
                    strokeDasharray={`${unit} ${unit}`}
                    className={`${line.stroke} stroke-1`}
                  />
                  <text
                    x={padLeft + plotWidth}
                    y={y(line.tokens) - unit / 2}
                    textAnchor="end"
                    className={`${line.fill} text-2xs`}
                  >
                    {line.id} {formatTokens(line.tokens)}
                  </text>
                </g>
              ))}
            {log.rebirths.map((rebirth) => (
              <g key={rebirth.streamSeq}>
                <line
                  x1={x(rebirth.after)}
                  x2={x(rebirth.after)}
                  y1={padTop}
                  y2={padTop + plotHeight}
                  className="stroke-link stroke-1"
                />
                <text
                  x={x(rebirth.after) + unit / 2}
                  y={padTop + unit * 2}
                  className="fill-link text-2xs"
                >
                  G{rebirth.record.generation}
                </text>
              </g>
            ))}
            {log.context.map((point) => (
              <circle
                key={`${point.atMs}-${point.after}`}
                cx={x(point.after)}
                cy={y(point.usedTokens)}
                r={unit}
                className="fill-foreground stroke-background stroke-2"
              />
            ))}
          </svg>
        )}
        {n === 0 && (
          <p className="text-muted-foreground absolute inset-0 flex items-center justify-center text-xs">
            No context injections yet.
          </p>
        )}
        {hovered && hoveredTotals && hover !== null && (
          <div
            role="status"
            className="bg-popover text-popover-foreground rounded-surface pointer-events-none absolute top-0 z-10 flex max-w-xs flex-col gap-0.5 border px-2 py-1.5 text-xs"
            style={
              x(hover) > size.width / 2
                ? { right: `${size.width - x(hover)}px` }
                : { left: `${x(hover + 1)}px` }
            }
          >
            <span className="font-medium">
              #{hover + 1} · {KIND_LABELS[hovered.injection.kind]} · +
              {formatTokens(hovered.injection.tokensEstimate)}
            </span>
            <span className="text-muted-foreground truncate">{hovered.injection.label}</span>
            <span className="tabular-nums">
              Injected into generation {log.generationOf[hover] ?? 0}:{" "}
              {formatTokens(sumOf(hoveredTotals))} tokens
            </span>
          </div>
        )}
      </div>
      <ul className="flex flex-wrap gap-x-3 gap-y-1 text-xs" aria-label="Injected tokens by kind">
        {GROUPS.map((group) => (
          <li key={group.id} className="flex items-center gap-1.5">
            <span aria-hidden className={`${group.swatch} size-2 rounded-xs`} />
            <span>{group.label}</span>
            <span className="text-muted-foreground tabular-nums">
              {formatTokens(log.totals[group.id])} · {log.counts[group.id]}×
            </span>
          </li>
        ))}
        <li className="flex items-center gap-1.5">
          <span aria-hidden className="bg-foreground size-2 rounded-full" />
          <span>Context the CLI reported</span>
        </li>
        {thresholds && (
          <li className="flex items-center gap-1.5">
            <span aria-hidden className="border-warning w-3 border-t border-dashed" />
            <span>Prepare</span>
            <span aria-hidden className="border-destructive w-3 border-t border-dashed" />
            <span>Swap</span>
          </li>
        )}
        {log.rebirths.length > 0 && (
          <li className="flex items-center gap-1.5">
            <span aria-hidden className="bg-link h-3 w-px" />
            <span>Rebirth</span>
            <span className="text-muted-foreground tabular-nums">{log.rebirths.length}×</span>
          </li>
        )}
      </ul>
    </div>
  );
}
