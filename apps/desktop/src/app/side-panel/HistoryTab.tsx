import { useState } from "react";

import { Timeline } from "@/components/conversation";
import { Button } from "@/components/controls";
import { formatDuration } from "@/lib/format";
import { useNow } from "@/hooks/use-now";
import { isNativeWorker } from "@/state/nativeWorkers";
import { useWorkerRows } from "@/state/cliThread";
import { rowName } from "@/app/conversation/activity/workerState";
import { loadBackgroundHistory, useBackgroundHistory, type BackgroundHistoryRecord } from "@/state/backgroundHistory";
import { askWorkerPanel } from "@/state/workerPanel";
import { CHAT_TAB, selectTab } from "@/state/sessionTabs";
import { HistoricalOutput } from "@/app/side-panel/OutputTab";

/** Whole-session lifecycle evidence on the existing Timeline, newest starts first. */
export function HistoryTab({ conversationId }: { conversationId: string }) {
  const history = useBackgroundHistory(conversationId);
  const workers = useWorkerRows(conversationId);
  const [opened, setOpened] = useState<Set<string>>(new Set());
  const now = useNow(history.records.some((record) => record.status === "running") ? 1000 : null);
  return (
    <div data-slot="background-history" className="flex min-h-0 flex-1 flex-col overflow-y-auto">
      <div className="max-w-thread mx-auto flex w-full flex-col gap-5 px-5 pt-6 pb-10">
        <h2 className="text-sm font-medium">Everything that ran this session{history.total !== null && ` · ${history.total}`}</h2>
        {!history.loaded && !history.error && <p role="status" className="text-muted-foreground text-sm">Loading background history…</p>}
        {history.loaded && history.records.length === 0 && <p className="text-muted-foreground text-sm">Background commands, subagents and previews will appear here as they run.</p>}
        {history.error && <div role="alert" className="flex items-center gap-2 text-sm"><p className="text-destructive">Couldn't load background history: {history.error}</p><Button size="xs" variant="ghost" disabled={history.busy} onClick={() => void loadBackgroundHistory(conversationId)}>Retry</Button></div>}
        <Timeline events={history.records.map((record) => {
          const isOpen = opened.has(record.id);
          const agent = record.kind === "agent" ? workers.find((row) => isNativeWorker(row) && (row.native.backgroundId === record.taskId || row.native.callId === record.callId)) : undefined;
          return {
            id: record.id,
            when: record.status === "running" || record.status === "paused" ? "now" : "past",
            failed: record.status === "failed" || (record.exitCode !== null && record.exitCode !== 0),
            time: new Date(record.startedAtMs).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }),
            title: <button type="button" aria-expanded={isOpen} aria-controls={`background-${record.id}`} className="hover:underline rounded-control text-start underline-offset-4"
              onClick={() => setOpened((shown) => { const next = new Set(shown); if (isOpen) next.delete(record.id); else next.add(record.id); return next; })}>{record.purpose || "Background command"}</button>,
            detail: <><span>{record.workerId ? rowName(workers, record.workerId) ?? `Worker ${record.workerId.slice(0, 8)}` : "Session"}</span>{" · "}{historyOutcome(record)}{" · "}{historyDuration(record, now)}<span> · {new Date(record.startedAtMs).toLocaleDateString()}</span></>,
            more: isOpen ? <div id={`background-${record.id}`} className="flex min-w-0 flex-col gap-3 pt-2"><HistoricalOutput conversationId={conversationId} recordId={record.id} initial={record} inline />{agent && <Button size="xs" variant="ghost" onClick={() => { selectTab(conversationId, CHAT_TAB); askWorkerPanel(conversationId, agent.id); }}>Open its thread</Button>}</div> : undefined,
          };
        })} />
        {history.next !== null && <Button variant="ghost" size="sm" disabled={history.busy} onClick={() => void loadBackgroundHistory(conversationId, true)}>{history.busy ? "Loading…" : "Load older activity"}</Button>}
      </div>
    </div>
  );
}

export function historyOutcome(record: BackgroundHistoryRecord): string {
  if (record.status === "unknown") return "End not observed";
  if (record.status === "stopped") return record.stopActor === "user" ? "Stopped by you" : "Stopped";
  if (record.exitCode !== null) return `${record.exitCode === 0 ? "Finished" : "Failed"} · exit ${record.exitCode}`;
  return ({ running: "Running", paused: "Paused", completed: "Finished", failed: "Failed" } as Record<string, string>)[record.status] ?? "End not observed";
}
export function historyDuration(record: BackgroundHistoryRecord, now: number): string {
  if (record.endedAtMs !== null) return formatDuration(Math.max(0, record.endedAtMs - record.startedAtMs));
  if (record.status === "running" || record.status === "paused") return `${formatDuration(Math.max(0, now - record.startedAtMs))} so far`;
  return "Duration unknown";
}
