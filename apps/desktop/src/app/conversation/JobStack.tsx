import { JobCard, RunJobCard } from "@/app/conversation/cards/JobCard";
import { usePlanBar, useSessionJobs } from "@/app/conversation/boardJob";
import { useLiveJob } from "@/app/conversation/liveJob";
import { overnightActions, useOvernightCards } from "@/app/conversation/overnightAdapter";
import { runInSlot } from "@/app/conversation/runJob";
import { revealOvernight } from "@/app/conversation/summaryState";
import { openPanelTab } from "@/state/sessionTabs";
import { useNow } from "@/hooks/use-now";

/**
 * The session's Job card, only for a job of several steps: an overnight run, in every state of
 * the run, or a plan, its phase bar under what is happening right now (J1). Never otherwise: the
 * session's state is the tab's and the sidebar's, its CI and merges are the
 * Background rows', buttons included. What the session waits for at a set time is the "Resumes
 * at" card's (`ResumesAtCard`).
 */
export function JobStack({ conversationId }: { conversationId: string }) {
  const job = useSessionJobs(conversationId)?.current ?? null;
  const run = runInSlot(useOvernightCards(conversationId), job);
  const live = useLiveJob(conversationId);
  const plan = usePlanBar(conversationId);
  const nowMs = useNow((run || plan) && live?.tone === "running" ? 1_000 : null);
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
    </>
  );
}
