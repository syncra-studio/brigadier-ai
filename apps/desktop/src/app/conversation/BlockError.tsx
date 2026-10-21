import type { FC } from "react";

import { isRunRequest } from "@/app/conversation/blocks";
import type { BlockMeta } from "@/app/conversation/RequestBlock";
import { describeTurnError, interruptedByQuit } from "@/app/conversation/turnError";
import { UsageLimitDivider } from "@/app/conversation/UsageLimitDivider";
import { useAction } from "@/app/conversation/useAction";
import { useViewConversation } from "@/app/conversation/viewContext";
import { ErrorState, useAui, useAuiState } from "@/components/conversation";
import { VENDOR_LABELS } from "@/lib/routing";
import { resume, send } from "@/state/actions";

/** What a session's Try again sends, as the user's own message. */
const TRY_AGAIN = "Try again.";

/**
 * The block's error: one plain line, the error's own words under Details, and Try again where it
 * can be: a Chat answers again; a session's last turn gets "Try again." sent as the user's
 * message, the way the composer sends it. An overnight run's turn has none: the run goes on.
 */
export const BlockError: FC<{ meta: BlockMeta; live: boolean }> = ({ meta, live }) => {
  const error = useAuiState((s) => {
    const status = s.message.status;
    return status?.type === "incomplete" && status.reason === "error"
      ? String(status.error ?? "The reply failed.")
      : null;
  });
  const latest = useAuiState((s) => s.message.isLast);
  const conversation = useViewConversation();
  const aui = useAui();
  const action = useAction();
  if (error === null) return null;
  const model = meta.texts.at(-1)?.model ?? meta.picked;
  const { summary, limit } = describeTurnError(error, model ? VENDOR_LABELS[model.provider] : undefined);
  // Brigadier quit while it ran: Continue picks the request up (UI audit H1).
  const interrupted = interruptedByQuit(error);
  const again = (() => {
    if (interrupted) {
      if (live || !latest || !conversation || conversation.lifecycle === "archived" || action.busy) return undefined;
      const id = conversation.id;
      return () => action.run(() => resume(id));
    }
    if (live || isRunRequest(meta.requestId)) return undefined;
    if (!meta.session) return meta.rework ? () => aui.message().reload() : undefined;
    if (!latest || !conversation || conversation.lifecycle === "archived" || action.busy) return undefined;
    const to = conversation.id;
    return () => action.run(() => send({ text: TRY_AGAIN, attachments: [], mentions: [] }, undefined, { to }));
  })();
  // A usage limit is no error of the turn: a divider says it, with Try again for later.
  if (limit) {
    return <UsageLimitDivider summary={summary} onTryAgain={again} />;
  }
  return (
    <div className="flex flex-col gap-1">
      <ErrorState summary={summary} details={interrupted ? null : error} onRetry={again} retryLabel={interrupted ? "Continue" : undefined} />
      {action.error && (
        <p role="alert" className="text-muted-foreground px-3 text-xs">
          Couldn’t send it: {action.error}
        </p>
      )}
    </div>
  );
};
