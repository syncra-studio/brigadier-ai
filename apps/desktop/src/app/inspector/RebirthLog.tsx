import { ChevronRight } from "@openai/apps-sdk-ui/components/Icon";
import { useState } from "react";

import { formatTokens, type RebirthRow } from "@/app/inspector/orchestratorLog";
import { Spinner } from "@/components/glyphs/spinner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { request } from "@/ipc/client";
import type { RebirthRecord } from "@/ipc/generated";
import { formatClock, formatDuration } from "@/lib/format";
import { cn } from "@/lib/utils";

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** A stored text (the handoff note, the briefing), read when the user opens it. */
function BlobText({ label, hash }: { label: string; hash: string }) {
  const [text, setText] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const toggle = () => {
    if (open) {
      setOpen(false);
      return;
    }
    setOpen(true);
    if (text !== null || loading) return;
    setLoading(true);
    setError(null);
    request({ method: "readBlobText", hash })
      .then((response) => setText(response.text))
      .catch((cause: unknown) => setError(errorText(cause)))
      .finally(() => setLoading(false));
  };
  return (
    <div className="flex flex-col gap-1">
      <Button size="xs" variant="ghost" className="self-start" aria-expanded={open} onClick={toggle}>
        <ChevronRight className={cn("transition-transform", open && "rotate-90")} />
        {label}
        {loading && <Spinner className="animate-spin" />}
      </Button>
      {open && error && (
        <p role="alert" className="text-destructive">
          {error}
        </p>
      )}
      {open && text !== null && (
        <pre className="bg-muted/50 rounded-control max-h-80 overflow-auto p-2 font-mono whitespace-pre-wrap wrap-break-word">
          {text}
        </pre>
      )}
    </div>
  );
}

const TRIGGERS: Record<RebirthRecord["trigger"], string> = {
  threshold: "threshold",
  recovery: "recovery",
  cacheExpired: "cache expired",
};

/** A wait shorter than this is not worth a mention. */
const WAIT_SHOWN_MS = 1000;

/**
 * Where a rebirth's time went: writing the handoff note, then the swap (the old CLI retired,
 * the briefing built for the new one), and between them the wait for the orchestrator's next
 * turn, which is idle time, not work. A record made before the swap's start was kept has only
 * the whole span.
 */
type RebirthTimes =
  | { kind: "split"; note: number | null; swap: number; waited: number }
  | { kind: "span"; span: number };

function rebirthTimes(record: RebirthRecord): RebirthTimes {
  const swapStarted = record.swapStartedAtMs;
  if (swapStarted === null) {
    return { kind: "span", span: record.swappedAtMs - record.prepareStartedAtMs };
  }
  const ready = record.handoffReadyAtMs;
  if (ready === null) {
    return { kind: "split", note: null, swap: record.swappedAtMs - swapStarted, waited: 0 };
  }
  // A swap that could not wait any longer began before the note was ready and waited for it.
  return {
    kind: "split",
    note: ready - record.prepareStartedAtMs,
    swap: record.swappedAtMs - Math.max(swapStarted, ready),
    waited: Math.max(0, swapStarted - ready),
  };
}

/** Rebirth steps take seconds; tenths are enough. */
function seconds(ms: number): string {
  return `${(ms / 1000).toFixed(1)} s`;
}

function tookText(times: RebirthTimes): string {
  if (times.kind === "span") return `prepared to swapped in ${seconds(times.span)}`;
  const work = (times.note ?? 0) + times.swap;
  const waited =
    times.waited >= WAIT_SHOWN_MS ? ` · waited ${formatDuration(times.waited)} for the next turn` : "";
  return `took ${seconds(work)}${waited}`;
}

function RebirthDetail({ record }: { record: RebirthRecord }) {
  const times = rebirthTimes(record);
  return (
    <div className="flex flex-col gap-2 px-3 pb-3">
      {times.kind === "split" && (
        <p className="text-muted-foreground tabular-nums">
          {times.note !== null &&
            (record.handoffBlob
              ? `handoff note ${seconds(times.note)} · `
              : `no handoff note (tried for ${seconds(times.note)}) · `)}
          swap {seconds(times.swap)}
          {times.waited >= WAIT_SHOWN_MS && " · the wait for the next turn is not counted"}
        </p>
      )}
      <p className="text-muted-foreground">
        {record.provider}
        {record.model && ` · ${record.model}`}
        {record.windowTokens !== null && ` · window ${formatTokens(record.windowTokens)}`}
        {" · "}
        <span className="font-mono">{record.oldNativeId ?? "?"}</span> →{" "}
        <span className="font-mono">{record.newNativeId ?? "not started yet"}</span>
      </p>
      <table className="w-full">
        <thead className="text-muted-foreground">
          <tr className="border-b">
            <th className="py-1 text-start font-medium">Briefing section</th>
            <th className="px-2 py-1 text-end font-medium">Tokens</th>
            <th className="px-2 py-1 text-end font-medium">Items</th>
            <th className="py-1 text-start font-medium">Cut to fit</th>
          </tr>
        </thead>
        <tbody>
          {record.sections.map((section) => (
            <tr key={section.name} className="border-b align-top last:border-b-0">
              <td className="py-1 font-mono">{section.name}</td>
              <td className="px-2 py-1 text-end tabular-nums">{formatTokens(section.tokens)}</td>
              <td className="px-2 py-1 text-end tabular-nums">{section.items}</td>
              <td className={cn("py-1", section.truncated ? "text-warning" : "text-muted-foreground")}>
                {section.truncated ? (section.note ?? "yes") : "no"}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {record.handoffBlob ? (
        <BlobText label="Handoff note" hash={record.handoffBlob} />
      ) : (
        <p className="text-warning">The outgoing orchestrator wrote no handoff note.</p>
      )}
      <BlobText label={`Briefing · ${formatTokens(record.briefingTokens)} tokens`} hash={record.briefingBlob} />
    </div>
  );
}

function RebirthRowView({ row }: { row: RebirthRow }) {
  const [open, setOpen] = useState(false);
  const { record } = row;
  return (
    <li className="border-b last:border-b-0">
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
        className="hover:bg-accent/50 flex w-full flex-col gap-0.5 px-3 py-2 text-start"
      >
        <span className="flex items-center gap-2">
          <ChevronRight
            aria-hidden
            className={cn("text-muted-foreground size-icon-xs shrink-0 transition-transform", open && "rotate-90")}
          />
          <span className="font-medium">Generation {record.generation}</span>
          <Badge variant={record.trigger === "threshold" ? "secondary" : "warning"}>
            {TRIGGERS[record.trigger]}
          </Badge>
          <span className="flex-1" />
          <span className="text-muted-foreground tabular-nums">{formatClock(row.atMs)}</span>
        </span>
        <span className="text-muted-foreground ps-5 tabular-nums">
          at {formatTokens(record.atTokens)} tokens · briefing {formatTokens(record.briefingTokens)} ·{" "}
          {record.decisions} decisions ({record.decisionsInFull} in full) · {record.recentMessages} recent
          messages · {tookText(rebirthTimes(record))}
        </span>
      </button>
      {open && <RebirthDetail record={record} />}
    </li>
  );
}

/** One row per orchestrator rebirth, newest first; each opens to its briefing. */
export function RebirthLog({ rebirths }: { rebirths: readonly RebirthRow[] }) {
  return (
    <div data-selectable className="min-h-0 flex-1 overflow-y-auto text-xs">
      {rebirths.length === 0 ? (
        <p className="text-muted-foreground p-4">
          The orchestrator has not been reborn yet. It is, between turns, once its context passes
          the swap line.
        </p>
      ) : (
        <ul>
          {rebirths.toReversed().map((row) => (
            <RebirthRowView key={row.streamSeq} row={row} />
          ))}
        </ul>
      )}
    </div>
  );
}
