import { useState } from "react";

import { OfferBody, OfferCard, OfferError, offerButton } from "@/app/conversation/OfferCard";
import { useAction } from "@/app/conversation/useAction";
import { Branch, Check } from "@/components/icons";
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
    <OfferCard
      label="Start as session?"
      kind="sessionOffered"
      icon={started ? <Check className="text-success" /> : <Branch />}
      title={started ? "Started as a session" : "Start this as its own session?"}
      line="Its own branch and worktree, in this project"
      actions={
        started ? (
          <button type="button" onClick={onOpen} className={offerButton("soft")}>
            Open
          </button>
        ) : (
          <button type="button" disabled={busy} onClick={onStart} className={offerButton("primary")}>
            Start as session
          </button>
        )
      }
    >
      <OfferBody title={title} text={task} />
      {error && <OfferError>{error}</OfferError>}
    </OfferCard>
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
