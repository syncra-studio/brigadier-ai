import { useMemo } from "react";

import { JobCard, WaitCard } from "@/app/conversation/cards/JobCard";
import { useSessionJobs } from "@/app/conversation/boardJob";
import { workerName } from "@/app/conversation/rowWords";
import { openPanelTab } from "@/state/sessionTabs";
import { formatTime } from "@/lib/format";
import { useBoard } from "@/state/board";
import { useNow } from "@/hooks/use-now";
import { useApp } from "@/state/store";

/** Something the session waits for at a set time: a usage limit's reset, a run's set end. */
type Wait = { key: string; title: string; reason?: string | undefined; atMs: number };

/** The overnight run states whose set end is still ahead. */
const RUN_ENDS = new Set(["running", "waitingQuota"]);

/**
 * What the session waits for at a set time, only where the board holds the time: the thread or
 * its workers paused until a usage limit resets, an overnight run's set end.
 */
function useWaits(conversationId: string): Wait[] {
  const lead = useApp((s) => s.conversations[conversationId]?.quotaWait ?? null);
  const json = useBoard((s) => {
    if (s.board?.conversationId !== conversationId) return "[]";
    const waits: Wait[] = [];
    if (lead?.resetsAtMs) waits.push({ key: "lead", title: `Resumes at ${formatTime(lead.resetsAtMs)}`, reason: lead.reason, atMs: lead.resetsAtMs });
    const paused = Object.values(s.board.tasks)
      .filter((task) => task.quotaWait?.resetsAtMs)
      .toSorted((a, b) => a.quotaWait!.resetsAtMs! - b.quotaWait!.resetsAtMs!);
    const first = paused[0];
    if (first && !lead) {
      const at = formatTime(first.quotaWait!.resetsAtMs!);
      const title = paused.length === 1 ? `${workerName(s.board.tasks, first)} resumes at ${at}` : `${paused.length} workers resume at ${at}`;
      waits.push({ key: "workers", title, reason: first.quotaWait!.reason, atMs: first.quotaWait!.resetsAtMs! });
    }
    for (const run of Object.values(s.board.overnight)) {
      if (run.windDownAtMs && RUN_ENDS.has(run.state)) {
        waits.push({ key: `run-${run.id}`, title: `The overnight run wraps up at ${formatTime(run.windDownAtMs)}`, reason: run.name, atMs: run.windDownAtMs });
      }
    }
    return JSON.stringify(waits);
  });
  return useMemo(() => JSON.parse(json) as Wait[], [json]);
}

/**
 * The session's job while it works through steps (Q88), then what it waits for at a set time:
 * cards of their own, under the context card or in its place while it is hidden.
 */
export function JobStack({ conversationId }: { conversationId: string }) {
  const job = useSessionJobs(conversationId)?.current ?? null;
  const waits = useWaits(conversationId);
  const nowMs = useNow(waits.length > 0 ? 30_000 : null);
  return (
    <>
      {job && <JobCard job={job} onOpen={() => openPanelTab(conversationId, { type: "reviews" }, "Progress")} />}
      {waits.map((wait) => (
        <WaitCard key={wait.key} title={wait.title} reason={wait.reason} atMs={wait.atMs} nowMs={nowMs} />
      ))}
    </>
  );
}
