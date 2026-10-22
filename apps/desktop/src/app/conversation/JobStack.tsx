import { useMemo } from "react";

import { JobCard, RunJobCard, WaitCard } from "@/app/conversation/cards/JobCard";
import { usePlanBar, useSessionJobs } from "@/app/conversation/boardJob";
import { useLiveJob } from "@/app/conversation/liveJob";
import { overnightActions, useOvernightCards } from "@/app/conversation/overnightAdapter";
import { runInSlot } from "@/app/conversation/runJob";
import { revealOvernight } from "@/app/conversation/summaryState";
import { workerName } from "@/app/conversation/rowWords";
import { openPanelTab } from "@/state/sessionTabs";
import { formatTime } from "@/lib/format";
import { useBoard } from "@/state/board";
import { useNow } from "@/hooks/use-now";
import { useApp } from "@/state/store";

/** Something the session waits for at a set time: a usage limit's reset. */
type Wait = { key: string; title: string; reason?: string | undefined; atMs: number };

/**
 * What the session waits for at a set time, only where the board holds the time: the thread or
 * its workers paused until a usage limit resets. An overnight run's end is its job card's line.
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
    return JSON.stringify(waits);
  });
  return useMemo(() => JSON.parse(json) as Wait[], [json]);
}

/**
 * The session's Job card, only for a job of several steps: an overnight run, in every state of
 * the run, or a plan, its phase bar under what is happening right now (J1). Never otherwise: the
 * session's state is the tab's and the sidebar's, its checks, reviews, CI and merges are the
 * Background rows', buttons included. Then what the session waits for at a set time: cards of
 * their own, under the context card or in its place while it is hidden.
 */
export function JobStack({ conversationId }: { conversationId: string }) {
  const job = useSessionJobs(conversationId)?.current ?? null;
  const run = runInSlot(useOvernightCards(conversationId), job);
  const live = useLiveJob(conversationId);
  const plan = usePlanBar(conversationId);
  const waits = useWaits(conversationId);
  const nowMs = useNow((run || plan) && live?.tone === "running" ? 1_000 : waits.length > 0 ? 30_000 : null);
  const open = () => openPanelTab(conversationId, { type: "reviews" }, "Progress");
  return (
    <>
      {run ? (
        <RunJobCard
          key={run.run.id}
          model={run}
          actions={overnightActions}
          live={live}
          nowMs={nowMs}
          onOpen={open}
          onEdit={() => revealOvernight(run.run.id)}
        />
      ) : (
        plan && <JobCard live={live} plan={plan} nowMs={nowMs} onOpen={open} />
      )}
      {waits.map((wait) => (
        <WaitCard key={wait.key} title={wait.title} reason={wait.reason} atMs={wait.atMs} nowMs={nowMs} />
      ))}
    </>
  );
}
