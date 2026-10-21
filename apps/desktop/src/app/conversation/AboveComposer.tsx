import { type FC, useContext, useMemo, useState } from "react";

import { ABOVE_LINE } from "@/app/conversation/aboveLine";
import { ArchivedBar } from "@/app/conversation/ArchivedBar";
import { Notices } from "@/app/conversation/Notices";
import { newestFailedError, withoutTurnError } from "@/app/conversation/turnError";
import { useAction } from "@/app/conversation/useAction";
import { ViewContext } from "@/app/conversation/viewContext";
import { Warning, X } from "@/components/icons";
import type { Notice } from "@/ipc/generated";
import { loadConversation, restore } from "@/state/actions";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";

const NO_NOTICES: Notice[] = [];

/** Between the thread and the composer: notices, a failed run, the archived state. */
export const AboveComposer: FC = () => {
  const { conversation } = useContext(ViewContext);
  const conversationId = conversation?.id ?? null;
  const notices = useBoard((s) =>
    s.board?.conversationId === conversationId ? s.board.notices : NO_NOTICES,
  );
  // A run that failed a request is said by that request in the thread, once.
  const runError = useBoard((s) =>
    s.board?.conversationId === conversationId && s.board.run === "failed" && !s.board.runRequest
      ? (s.board.runError ?? "The model stopped with an error.")
      : null,
  );
  // What the newest failed turn already says in the thread is never said again here.
  const turnError = useBoard((s) =>
    s.board?.conversationId === conversationId ? newestFailedError(s.board.requests) : null,
  );
  const shown = useMemo(() => withoutTurnError(notices, turnError), [notices, turnError]);
  const failure = runError !== null && runError.trim() !== turnError?.trim() ? runError : null;
  const [failureDismissed, setFailureDismissed] = useState<string | null>(null);
  const action = useAction();
  // Its messages could not be loaded: said once, with Retry.
  const loadError = useApp((s) => (conversationId ? (s.threads[conversationId]?.loadError ?? null) : null));
  const reload = useAction();
  const archived = conversation?.lifecycle === "archived";
  const failed = failure !== null && failure !== failureDismissed;
  // Nothing at all when there is nothing to say, so the footer adds no gap for it.
  if (!conversation || (shown.length === 0 && !failed && !archived && loadError === null)) return null;
  return (
    <div className="flex flex-col gap-1.5">
      {loadError !== null && (
        <p data-slot="load-error" role="alert" title={loadError} className={ABOVE_LINE}>
          <Warning aria-hidden className="text-muted-foreground size-icon-xs mt-px shrink-0" />
          <span className="min-w-0 flex-1">Couldn’t load messages</span>
          <button
            type="button"
            disabled={reload.busy}
            className="text-link hover:text-link/80 shrink-0"
            onClick={() => reload.run(() => loadConversation(conversation.id))}
          >
            Retry
          </button>
        </p>
      )}
      <Notices notices={shown} />
      {failed && (
        <p role="status" className={ABOVE_LINE}>
          <Warning aria-hidden className="text-muted-foreground size-icon-xs mt-px shrink-0" />
          <span className="min-w-0 flex-1 whitespace-pre-wrap">{failure}</span>
          <button
            type="button"
            aria-label="Dismiss"
            className="text-muted-foreground hover:text-foreground shrink-0"
            onClick={() => setFailureDismissed(failure)}
          >
            <X className="size-icon-xs" />
          </button>
        </p>
      )}
      {archived ? (
        <ArchivedBar
          error={action.error}
          busy={action.busy}
          onRestore={() => action.run(() => restore(conversation.id))}
        />
      ) : null}
    </div>
  );
};
