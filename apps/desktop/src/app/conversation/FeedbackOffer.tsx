import { createContext, useContext, useEffect, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { field } from "@/components/assistant-ui/elements/surfaces";
import { FeedbackGlyph } from "@/components/glyphs/feedback-glyph";
import { ArrowUpRight, Check } from "@/components/ui/icon";
import { detailsLine, type FeedbackKind } from "@/lib/feedbackUrl";
import { cn } from "@/lib/utils";
import { answerFeedbackOffer } from "@/state/actions";
import { useBoard } from "@/state/board";
import { feedbackDetails, openFeedback, sendFeedback } from "@/state/feedback";

const KIND_LABEL: Record<FeedbackKind, string> = { bug: "Something’s wrong", idea: "An idea" };

export type FeedbackOfferState = "open" | "sent" | "dismissed";

/**
 * The thread's offer of a report to Brigadier's developers, drafted from the conversation when
 * the user seemed annoyed with Brigadier (its `offer_feedback`): the draft, what goes with it,
 * and Open on GitHub, Edit first (Send feedback, filled in) or Dismiss. Nothing leaves the app
 * before a click; once sent it says where to finish, once dismissed it is gone.
 */
export function FeedbackOfferCard({
  kind,
  title,
  body,
  state,
  busy = false,
  error,
  onSend,
  onEdit,
  onDismiss,
}: {
  kind: FeedbackKind;
  title: string;
  body: string;
  state: FeedbackOfferState;
  busy?: boolean;
  error?: string | null;
  onSend: () => void;
  onEdit: () => void;
  onDismiss: () => void;
}) {
  const [details, setDetails] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    void feedbackDetails().then((found) => live && setDetails(detailsLine(found) || null));
    return () => {
      live = false;
    };
  }, []);
  if (state === "dismissed") return null;
  return (
    <section
      aria-label="Send this as feedback?"
      data-slot="orchestrator-step"
      data-kind="feedbackOffered"
      className="bg-rail border-foreground/10 rounded-question my-1 flex flex-col gap-3 border p-4 text-sm"
    >
      {state === "sent" ? (
        <p role="status" className="flex items-center gap-2.5">
          <Check className="text-success size-icon-md shrink-0" />
          Opened on GitHub: finish it there and post it.
        </p>
      ) : (
        <>
          <div className="flex min-w-0 items-center gap-2.5">
            <span className="bg-foreground/5 text-muted-foreground rounded-control size-icon-button-md flex shrink-0 items-center justify-center">
              <FeedbackGlyph className="size-icon-sm" />
            </span>
            <div className="min-w-0 flex-1">
              <p className="font-medium">Send this as feedback?</p>
              <p className="text-muted-foreground text-xs">
                {KIND_LABEL[kind]} · written from this chat, for Brigadier’s developers
              </p>
            </div>
          </div>
          <div className={cn(field, "rounded-settings flex flex-col gap-1 px-3 py-2.5")}>
            <p className="font-medium wrap-break-word">{title}</p>
            <p className="text-muted-foreground line-clamp-4 whitespace-pre-line wrap-break-word">{body}</p>
          </div>
          {details && <p className="text-foreground/50 text-xs">Adds {details}</p>}
          {error && (
            <p role="alert" className="text-destructive text-xs">
              {error}
            </p>
          )}
          <div className="flex flex-wrap items-center gap-2">
            <button
              type="button"
              disabled={busy}
              onClick={onSend}
              className="bg-foreground text-background rounded-capsule h-control-sm flex items-center gap-1.5 px-3.5 text-sm font-medium transition-opacity hover:opacity-90 active:scale-95 disabled:pointer-events-none disabled:opacity-40 motion-reduce:transition-none"
            >
              Open on GitHub
              <ArrowUpRight className="size-icon-sm" />
            </button>
            <button
              type="button"
              disabled={busy}
              onClick={onEdit}
              className={cn(
                field,
                "rounded-capsule h-control-sm hover:bg-foreground/10 px-3.5 text-sm transition-colors disabled:opacity-40",
              )}
            >
              Edit first
            </button>
            <button
              type="button"
              disabled={busy}
              onClick={onDismiss}
              className="text-muted-foreground hover:text-foreground rounded-capsule h-control-sm px-3 text-sm transition-colors disabled:opacity-40"
            >
              Dismiss
            </button>
          </div>
        </>
      )}
    </section>
  );
}

/**
 * Whether nothing of the request a step renders under runs any more (done, stopped or failed):
 * the offer waits for it, so it never comes mid-turn.
 */
export const TurnEnded = createContext(false);

/**
 * The offer `id` as the thread made it, answered through the daemon: Open on GitHub sends it
 * as #146's Send feedback does, Edit first opens that dialog filled in (sending there counts),
 * Dismiss puts it away for good.
 */
export function FeedbackOffer({ id, kind, title, body }: { id: string; kind: FeedbackKind; title: string; body: string }) {
  const ended = useContext(TurnEnded);
  const conversationId = useBoard((s) => s.board?.conversationId ?? null);
  const answered = useBoard((s) => {
    const step = s.board?.orchestratorSteps.find((each) => each.kind.type === "feedbackAnswered" && each.kind.id === id);
    return step?.kind.type === "feedbackAnswered" ? step.kind.sent : null;
  });
  const [answer, setAnswer] = useState<boolean | null>(null);
  const action = useAction();
  if (!ended || !conversationId) return null;
  const sent = answered ?? answer;
  const answerWith = (value: boolean) => answerFeedbackOffer(conversationId, id, value).then(() => setAnswer(value));
  const text = `${title}\n\n${body}`;
  return (
    <FeedbackOfferCard
      kind={kind}
      title={title}
      body={body}
      state={sent === null ? "open" : sent ? "sent" : "dismissed"}
      busy={action.busy}
      error={action.error}
      onSend={() => action.run(() => sendFeedback(kind, text).then(() => answerWith(true)))}
      onEdit={() => openFeedback({ kind, text, onSent: () => action.run(() => answerWith(true)) })}
      onDismiss={() => action.run(() => answerWith(false))}
    />
  );
}
