import type { ReactNode } from "react";

import { resumesAt } from "@/app/conversation/cards/resumesAt";
import { useAction } from "@/app/conversation/useAction";
import { ScheduleCard } from "@/components/conversation";
import { Button } from "@/components/controls";
import { Clock } from "@/components/icons";
import { useNow } from "@/hooks/use-now";
import { cn } from "@/lib/utils";
import { formatDateTime } from "@/lib/format";
import { continueWaiting, dontResume } from "@/state/actions";
import { useCliThread } from "@/state/cliThread";
import { useApp } from "@/state/store";

/** The conversation's one recovery notice, hosted above the composer rather than in the summary. */
export function QuotaWaitStrip({ conversationId }: { conversationId: string }) {
  const wait = useApp((s) => s.conversations[conversationId]?.quotaWait ?? null);
  const go = useAction();
  const stay = useAction();
  if (!wait) return null;
  const manual = wait.limitKind === "credits" || wait.limitKind === "spendControl";
  // Until the daemon supplies its recovery mode, a reset time describes the usage window,
  // not a promise that an automatic retry is armed.
  const words = wait.limitKind === "credits" ? "Out of credits"
    : wait.limitKind === "spendControl" ? "Spend limit reached"
    : wait.resetsAtMs === null ? "Usage limit reached · Reset time unavailable"
    : `Usage limit reached · Resets ${formatDateTime(wait.resetsAtMs)}`;
  return (
    <section data-slot="quota-wait-strip" role="status" aria-label="Usage limit"
      className="bg-rail/70 border-foreground/10 rounded-rail flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 border px-3 py-2 text-sm">
      <div className="flex min-w-0 flex-1 items-start gap-2">
        <Clock aria-hidden className="text-muted-foreground size-icon-sm mt-0.5 shrink-0" />
        <div className="min-w-0 wrap-break-word">
          <p>{words}</p>
          <p className="text-muted-foreground text-xs">{wait.reason}</p>
        </div>
      </div>
      <div className="flex flex-wrap gap-0.5">
        <Button size="xs" variant="ghost" className="text-muted-foreground font-normal" data-slot="continue-now"
          disabled={go.busy || stay.busy} onClick={() => go.run(() => continueWaiting(conversationId))}>
          Continue now
        </Button>
        {!manual && <Button size="xs" variant="ghost" className="text-muted-foreground font-normal" data-slot="dont-resume"
          disabled={go.busy || stay.busy} onClick={() => stay.run(() => dontResume(conversationId))}>
          Don't resume
        </Button>}
      </div>
      {(go.error ?? stay.error) && <p role="alert" className="text-destructive basis-full text-xs">{go.error ?? stay.error}</p>}
    </section>
  );
}

/**
 * A card of the summary's column beside the context card (Q88): the same surface, its own card.
 * It sits under the context card while that is open, and in its place while it is hidden.
 */
function StackCard({ className, children }: { className?: string; children: ReactNode }) {
  return (
    <div className={cn("bg-popover rounded-summary shadow-summary pointer-events-auto flex min-w-0 flex-col", className)}>
      {children}
    </div>
  );
}

/**
 * Something that waits on a set time, in the same column: a session paused until a usage limit
 * resets. Without a time, what it waits for.
 */
export function WaitCard({
  title,
  reason,
  atMs,
  nowMs,
  children,
}: {
  title: string;
  reason?: string | undefined;
  atMs?: number | null | undefined;
  nowMs: number;
  /** Its buttons, under its words. */
  children?: ReactNode;
}) {
  return (
    <StackCard>
      <ScheduleCard className={cn("px-5 py-3.5 text-label", children && "pb-1.5")} title={title} reason={reason} atMs={atMs} nowMs={nowMs} />
      {children && <div data-slot="wait-card-actions" className="flex flex-wrap gap-0.5 px-3.5 pb-2.5">{children}</div>}
    </StackCard>
  );
}

/** The session's "Resumes at" card, with Continue now and, while it resumes on its own, Don't resume. */
export function ResumesAtCard({ conversationId }: { conversationId: string }) {
  const wait = useApp((s) => s.conversations[conversationId]?.quotaWait ?? null);
  const limit = useCliThread((s) => s.views[conversationId]?.turns.at(-1)?.limit ?? null);
  const card = resumesAt(limit, wait);
  const nowMs = useNow(card?.atMs != null ? 30_000 : null);
  const go = useAction();
  const stay = useAction();
  if (!card) return null;
  return (
    <WaitCard title={card.title} reason={card.reason ?? undefined} atMs={card.atMs} nowMs={nowMs}>
      <Button size="xs" variant="ghost" className="text-muted-foreground font-normal" data-slot="continue-now"
        disabled={go.busy || stay.busy} onClick={() => go.run(() => continueWaiting(conversationId))}>
        Continue now
      </Button>
      {card.resumes && (
        <Button size="xs" variant="ghost" className="text-muted-foreground font-normal" data-slot="dont-resume"
          disabled={go.busy || stay.busy} onClick={() => stay.run(() => dontResume(conversationId))}>
          Don't resume
        </Button>
      )}
      {(go.error ?? stay.error) && <p role="alert" className="text-destructive basis-full px-1.5 text-xs">{go.error ?? stay.error}</p>}
    </WaitCard>
  );
}
