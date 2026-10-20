import { useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { field } from "@/components/assistant-ui/elements/surfaces";
import { Branch, Check } from "@/components/icons";
import { cn } from "@/lib/utils";
import { openConversation, startOfferedSession } from "@/state/actions";
import { useBoard } from "@/state/board";

/**
 * The thread's offer to start out-of-scope work as its own session (its `offer_session`): the
 * title, the task and Start as session. Only the click starts it, on its own worktree, and the
 * user stays here; once started the card says so and Open shows the new session.
 */
export function SessionOfferCard({
  title,
  task,
  started,
  busy = false,
  error,
  onStart,
  onOpen,
}: {
  title: string;
  task: string;
  started: boolean;
  busy?: boolean;
  error?: string | null;
  onStart: () => void;
  onOpen: () => void;
}) {
  return (
    <section
      aria-label="Start as session?"
      data-slot="orchestrator-step"
      data-kind="sessionOffered"
      className="bg-rail border-foreground/10 rounded-question my-1 flex flex-col gap-3 border p-4 text-sm"
    >
      <div className="flex min-w-0 items-center gap-2.5">
        <span className="bg-foreground/5 text-muted-foreground rounded-control size-icon-button-md flex shrink-0 items-center justify-center">
          {started ? <Check className="text-success size-icon-sm" /> : <Branch className="size-icon-sm" />}
        </span>
        <div className="min-w-0 flex-1">
          <p className="font-medium">{started ? "Started as a session" : "Start this as its own session?"}</p>
          <p className="text-muted-foreground text-xs">Its own branch and worktree, in this project</p>
        </div>
      </div>
      <div className={cn(field, "rounded-settings flex flex-col gap-1 px-3 py-2.5")}>
        <p className="font-medium wrap-break-word">{title}</p>
        <p className="text-muted-foreground line-clamp-4 whitespace-pre-line wrap-break-word">{task}</p>
      </div>
      {error && (
        <p role="alert" className="text-destructive text-xs">
          {error}
        </p>
      )}
      <div className="flex flex-wrap items-center gap-2">
        {started ? (
          <button
            type="button"
            onClick={onOpen}
            className={cn(field, "rounded-capsule h-control-sm hover:bg-foreground/10 px-3.5 text-sm transition-colors")}
          >
            Open
          </button>
        ) : (
          <button
            type="button"
            disabled={busy}
            onClick={onStart}
            className="bg-foreground text-background rounded-capsule h-control-sm flex items-center gap-1.5 px-3.5 text-sm font-medium transition-opacity hover:opacity-90 active:scale-95 disabled:pointer-events-none disabled:opacity-40 motion-reduce:transition-none"
          >
            Start as session
          </button>
        )}
      </div>
    </section>
  );
}

/**
 * The offer `id` as the thread made it, shown in place the moment it is made (its turn may
 * still work) and started through the daemon.
 */
export function SessionOffer({ id, title, task }: { id: string; title: string; task: string }) {
  const conversationId = useBoard((s) => s.board?.conversationId ?? null);
  const taken = useBoard((s) => {
    const step = s.board?.orchestratorSteps.find((each) => each.kind.type === "sessionOfferTaken" && each.kind.id === id);
    return step?.kind.type === "sessionOfferTaken" ? step.kind.session : null;
  });
  const [started, setStarted] = useState<string | null>(null);
  const action = useAction();
  if (!conversationId) return null;
  const session = taken ?? started;
  return (
    <SessionOfferCard
      title={title}
      task={task}
      started={session !== null}
      busy={action.busy}
      error={action.error}
      onStart={() => action.run(() => startOfferedSession(conversationId, id).then((conversation) => setStarted(conversation.id)))}
      onOpen={() => session && openConversation(session)}
    />
  );
}
