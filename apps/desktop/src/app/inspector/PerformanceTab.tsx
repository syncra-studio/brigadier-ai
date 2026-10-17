import { useEffect, useState, type ReactNode } from "react";

import { useDiagnosticsPolling } from "@/app/inspector/ProcessesTab";
import { Badge } from "@/components/ui/badge";
import type { Budget, DaemonMetrics, LatencySummary } from "@/ipc/generated";
import { formatBytes, formatDuration, formatMb, formatMs } from "@/lib/format";
import { frameGaps, ingestToPaint, summarize } from "@/lib/perf";
import { useApp } from "@/state/store";

type UiPerf = { ingest: LatencySummary; frames: LatencySummary };

function readUiPerf(): UiPerf {
  return {
    ingest: summarize(ingestToPaint.values()),
    frames: summarize(frameGaps.values()),
  };
}

/** Summaries of the UI-side samples, published twice a second (never per frame). */
function useUiPerf(): UiPerf {
  const [perf, setPerf] = useState(readUiPerf);
  useEffect(() => {
    const timer = setInterval(() => setPerf(readUiPerf()), 500);
    return () => clearInterval(timer);
  }, []);
  return perf;
}

/** The repository size the static index budget is stated for (PLAN.md §4). */
const STATIC_INDEX_FILES = 100_000;

type Evaluation = {
  measured: string;
  status: "pass" | "fail" | "pending" | "na";
  detail?: string;
};

function evaluate(
  budget: Budget,
  metrics: DaemonMetrics | null,
  perf: UiPerf,
  coldStartMs: number | null,
  firstLaunch: boolean,
  tolerance: number,
): Evaluation {
  if (budget.phase !== null) {
    return { measured: "—", status: "na", detail: `n/a — Phase ${budget.phase}` };
  }
  const limit =
    budget.limit === null ? null : budget.timing ? budget.limit * tolerance : budget.limit;
  const judge = (value: number | null, text: string, detail?: string): Evaluation => {
    if (value === null || limit === null) {
      return { measured: "—", status: "pending", detail: "waiting for samples" };
    }
    return {
      measured: text,
      status: value < limit ? "pass" : "fail",
      ...(detail === undefined ? {} : { detail }),
    };
  };
  switch (budget.id) {
    case "coreIdleRss":
      return metrics
        ? judge(metrics.rssBytes / (1024 * 1024), formatMb(metrics.rssBytes))
        : judge(null, "");
    case "coldStart":
    case "firstColdStart":
      if (firstLaunch !== (budget.id === "firstColdStart")) {
        return {
          measured: "—",
          status: "na",
          detail: firstLaunch ? "n/a — this was a first launch" : "n/a — not a first launch",
        };
      }
      return coldStartMs === null
        ? judge(null, "")
        : judge(coldStartMs, formatMs(coldStartMs), "process start → app shown and usable");
    case "ingestToPaint":
      return perf.ingest.samples === 0
        ? judge(null, "")
        : judge(
            perf.ingest.p95Ms,
            formatMs(perf.ingest.p95Ms),
            `${perf.ingest.samples} events, max ${formatMs(perf.ingest.maxMs)}`,
          );
    case "frameGaps":
      return perf.frames.samples === 0
        ? judge(null, "")
        : judge(
            perf.frames.maxMs,
            formatMs(perf.frames.maxMs),
            `largest of the last ${perf.frames.samples} frames`,
          );
    case "runtimeStalls":
      return metrics
        ? judge(
            metrics.tasks.slowPolls,
            String(metrics.tasks.slowPolls),
            `polls > ${formatMs(metrics.tasks.slowPollThresholdMs)} of ${metrics.tasks.polls}`,
          )
        : judge(null, "");
    case "schedulerDelay":
      return metrics && metrics.schedulerDelay.samples > 0
        ? judge(
            metrics.schedulerDelay.maxMs,
            formatMs(metrics.schedulerDelay.maxMs),
            `p95 ${formatMs(metrics.schedulerDelay.p95Ms)} over ${metrics.schedulerDelay.samples} heartbeats`,
          )
        : judge(null, "");
    case "brainQuery": {
      const query = metrics?.brain.query;
      if (!query) return judge(null, "");
      if (query.samples === 0) {
        return { measured: "—", status: "na", detail: "no query_brain calls yet" };
      }
      return judge(
        query.p95Ms,
        formatMs(query.p95Ms),
        `${query.samples} query_brain calls, max ${formatMs(query.maxMs)}`,
      );
    }
    case "staticIndex": {
      if (!metrics) return judge(null, "");
      const run = metrics.brain.largestIndexRun;
      if (!run) return { measured: "—", status: "na", detail: "no full index scan yet" };
      const scanned = `${run.files.toLocaleString()} files in ${formatMs(run.durationMs)}`;
      // The budget speaks of a 100k-file repo; a smaller scan says nothing about it.
      if (run.files < STATIC_INDEX_FILES) {
        return {
          measured: formatMs(run.durationMs),
          status: "na",
          detail: `largest scan: ${scanned}, under ${STATIC_INDEX_FILES.toLocaleString()} files`,
        };
      }
      return judge(run.durationMs, formatMs(run.durationMs), `largest scan: ${scanned}`);
    }
    default:
      return { measured: "—", status: "pending", detail: "not measured" };
  }
}

const STATUS: Record<Evaluation["status"], { label: string; variant: "success" | "destructive" | "secondary" | "outline" }> = {
  pass: { label: "pass", variant: "success" },
  fail: { label: "over", variant: "destructive" },
  pending: { label: "…", variant: "outline" },
  na: { label: "n/a", variant: "secondary" },
};

export function PerformanceTab() {
  useDiagnosticsPolling();
  const budgets = useApp((s) => s.inspector.diagnostics?.budgets ?? null);
  const metrics = useApp((s) => s.inspector.metrics);
  const coldStartMs = useApp((s) => s.coldStartMs);
  const firstLaunch = useApp((s) => s.info?.firstLaunch ?? false);
  const tolerance = useApp((s) => s.info?.budgetTolerance ?? 1);
  const perf = useUiPerf();

  return (
    <div data-selectable className="flex flex-col gap-4 pb-4">
      <table className="w-full text-xs">
        <thead className="text-muted-foreground">
          <tr className="h-row-sm border-b">
            <th className="px-3 text-start font-medium">Budget (§4)</th>
            <th className="px-2 text-start font-medium">Target</th>
            <th className="px-2 text-end font-medium">Measured</th>
            <th className="px-3 text-end font-medium">Status</th>
          </tr>
        </thead>
        <tbody>
          {(budgets ?? []).map((budget) => {
            const result = evaluate(budget, metrics, perf, coldStartMs, firstLaunch, tolerance);
            const status = STATUS[result.status];
            return (
              <tr key={budget.id} className="border-b align-top last:border-b-0">
                <td className="px-3 py-1.5">
                  <div>{budget.metric}</div>
                  {result.detail && (
                    <div className="text-muted-foreground text-2xs">{result.detail}</div>
                  )}
                </td>
                <td className="text-muted-foreground px-2 py-1.5">{budget.target}</td>
                <td className="px-2 py-1.5 text-end tabular-nums">{result.measured}</td>
                <td className="px-3 py-1.5 text-end">
                  <Badge variant={status.variant}>{status.label}</Badge>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
      {!budgets && <p className="text-muted-foreground px-3 text-xs">Loading budgets…</p>}
      {tolerance > 1 && (
        <p className="text-muted-foreground px-3 text-xs">
          Timing budgets are multiplied by {tolerance} (CI tolerance).
        </p>
      )}
      <DaemonDetails metrics={metrics} />
    </div>
  );
}

function Detail({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="text-end tabular-nums">{children}</dd>
    </>
  );
}

function DaemonDetails({ metrics }: { metrics: DaemonMetrics | null }) {
  if (!metrics) {
    return <p className="text-muted-foreground px-3 text-xs">Waiting for daemon metrics…</p>;
  }
  const { store, runtime, tasks } = metrics;
  return (
    <section className="px-3">
      <h2 className="text-muted-foreground mb-2 text-xs font-medium">brigadierd</h2>
      <dl className="grid grid-cols-2 gap-x-4 gap-y-1 text-xs">
        <Detail label="Uptime">{formatDuration(metrics.uptimeMs)}</Detail>
        <Detail label="RSS">{formatMb(metrics.rssBytes)}</Detail>
        <Detail label="CPU">{metrics.cpuPercent.toFixed(1)}%</Detail>
        <Detail label="Runtime workers">{runtime.workers}</Detail>
        <Detail label="Alive tasks">{runtime.aliveTasks}</Detail>
        <Detail label="Global queue depth">{runtime.globalQueueDepth}</Detail>
        <Detail label="Mean task poll">{tasks.meanPollUs.toFixed(0)} µs</Detail>
        <Detail label="Long scheduling delays">{tasks.longDelays}</Detail>
        <Detail label="Connections">{metrics.connections}</Detail>
        <Detail label="Last event seq">{store.lastSeq}</Detail>
        <Detail label="Queued writes">{store.queuedWrites}</Detail>
        <Detail label="Committed events">{store.committedEvents}</Detail>
        <Detail label="Last commit">{formatMs(store.lastCommitMs)}</Detail>
        <Detail label="Last batch">{store.lastBatchCommands} commands</Detail>
        <Detail label="WAL size">{formatBytes(store.walBytes)}</Detail>
        <Detail label="Checkpoints">{store.checkpoints}</Detail>
        <Detail label="Embedding model">
          {metrics.brain.embedderLoaded ? "loaded" : "not loaded"}
        </Detail>
        <Detail label="Undo failures">{metrics.undo.failed} of {metrics.undo.tried}</Detail>
      </dl>
    </section>
  );
}
