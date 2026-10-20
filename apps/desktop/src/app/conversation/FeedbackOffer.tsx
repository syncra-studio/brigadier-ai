import { createContext, useContext, useEffect, useState } from "react";

import { OfferBody, OfferCard, OfferError, offerButton } from "@/app/conversation/OfferCard";
import { useAction } from "@/app/conversation/useAction";
import { ArrowUpRight, Check, FeedbackGlyph } from "@/components/icons";
import { detailsLine, type FeedbackKind } from "@/lib/feedbackUrl";
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
  if (state === "sent") {
    return (
      <OfferCard label="Send this as feedback?" kind="feedbackOffered">
        <p role="status" className="flex items-center gap-2.5">
          <Check className="text-success size-icon-md shrink-0" />
          Opened on GitHub: finish it there and post it.
        </p>
      </OfferCard>
    );
  }
  return (
    <OfferCard
      label="Send this as feedback?"
      kind="feedbackOffered"
      icon={<FeedbackGlyph />}
      title="Send this as feedback?"
      line={`${KIND_LABEL[kind]} · written from this chat, for Brigadier’s developers`}
      actions={
        <>
          <button type="button" disabled={busy} onClick={onSend} className={offerButton("primary")}>
            Open on GitHub
            <ArrowUpRight className="size-icon-sm" />
          </button>
          <button type="button" disabled={busy} onClick={onEdit} className={offerButton("soft")}>
            Edit first
          </button>
          <button type="button" disabled={busy} onClick={onDismiss} className={offerButton("quiet")}>
            Dismiss
          </button>
        </>
      }
    >
      <OfferBody title={title} text={body} />
      {details && <p className="text-foreground/50 text-xs">Adds {details}</p>}
      {error && <OfferError>{error}</OfferError>}
    </OfferCard>
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
      onSend={() => action.run(() => sendFeedback(text, kind).then(() => answerWith(true)))}
      onEdit={() => openFeedback({ kind, text, onSent: () => action.run(() => answerWith(true)) })}
      onDismiss={() => action.run(() => answerWith(false))}
    />
  );
}
