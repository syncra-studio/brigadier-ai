import { useState } from "react";
import { Timeline, type TimelineEvent } from "@/components/conversation";
import { Button } from "@/components/controls";
import { openLink } from "@/app/conversation/summaryLinks";
import { forgetPr, refreshTrackedWork, useTrackedWork } from "@/app/conversation/trackedWork";
import { toast } from "@/state/toasts";

/** Separate from live rows: closed/merged/forgotten PRs retain all recorded transitions. */
export function PullRequestHistory({ sessionId }: { sessionId: string }) {
  const { work, loading, error } = useTrackedWork(sessionId);
  const [pending, setPending] = useState<string | null>(null);
  const events: TimelineEvent[] = work.prHistory.map((event) => {
    const pr = work.pullRequests.find((item) => item.identity.host === event.identity.host && item.identity.owner === event.identity.owner && item.identity.repo === event.identity.repo && item.identity.number === event.identity.number);
    const key = `${event.identity.host}/${event.identity.owner}/${event.identity.repo}/${event.identity.number}`;
    const url = `https://${event.identity.host}/${event.identity.owner}/${event.identity.repo}/pull/${event.identity.number}`;
    return {
      id: event.id,
      when: "past",
      time: new Date(event.atMs).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }),
      title: <button className="text-start hover:underline focus-visible:underline" onClick={() => openLink(url)}>{event.identity.repo} #{event.identity.number} · {event.action}</button>,
      detail: `${new Date(event.atMs).toLocaleDateString()} · ${event.actor}${event.account ? ` · ${event.account}` : ""} · ${event.detail}`,
      more: pr && !pr.forgotten && work.prHistory.find((item) => item.identity.host === event.identity.host && item.identity.owner === event.identity.owner && item.identity.repo === event.identity.repo && item.identity.number === event.identity.number)?.id === event.id ? (
        <Button variant="ghost" size="sm" disabled={pending === key} onClick={() => {
          setPending(key);
          void forgetPr(sessionId, key).catch((cause: unknown) => toast(cause instanceof Error ? cause.message : String(cause), { tone: "error" })).finally(() => setPending(null));
        }}>Forget pull request</Button>
      ) : undefined,
    };
  });
  return <div className="flex h-full min-h-0 flex-col overflow-y-auto p-4" data-slot="pr-history">
    <h2 className="mb-4 text-sm font-medium">Pull request history</h2>
    {loading && <p className="text-muted-foreground text-sm" role="status">Loading pull request history…</p>}
    {error && <div role="alert" className="mb-3 text-sm"><p>{error}</p><Button variant="ghost" size="sm" onClick={() => void refreshTrackedWork(sessionId)}>Try again</Button></div>}
    {!loading && !error && events.length === 0 && <p className="text-muted-foreground text-sm">No pull request history yet</p>}
    <Timeline events={events} />
  </div>;
}
