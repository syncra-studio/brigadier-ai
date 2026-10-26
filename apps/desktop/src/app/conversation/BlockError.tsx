import type { FC } from "react";
import { useShallow } from "zustand/react/shallow";

import { isLive, isRunRequest } from "@/app/conversation/blocks";
import type { BlockMeta } from "@/app/conversation/RequestBlock";
import { latestRequest } from "@/app/conversation/rework";
import { leadTransitions } from "@/app/conversation/providerTransitions";
import { cutOff, describeTurnError, interruptedByQuit } from "@/app/conversation/turnError";
import { UsageLimitDivider } from "@/app/conversation/UsageLimitDivider";
import { useAction } from "@/app/conversation/useAction";
import { useViewConversation } from "@/app/conversation/viewContext";
import { ErrorState, useAui, useAuiState } from "@/components/conversation";
import type { UserRequest } from "@/ipc/generated";
import { VENDOR_LABELS } from "@/lib/routing";
import { resume, send } from "@/state/actions";
import { useBoard } from "@/state/board";
import { useCliThread } from "@/state/cliThread";

/** Scope the conversation's current wait to its latest request; historical failures keep their diagnostics. */
export function useBlockQuotaRecovery(meta: BlockMeta | undefined) {
  const conversation = useViewConversation();
  const latest = useAuiState((s) => s.message.isLast);
  const error = useAuiState((s) => {
    const status = s.message.status;
    return status?.type === "incomplete" && status.reason === "error" ? String(status.error ?? "") : null;
  });
  const nativeLimit = useCliThread((s) => {
    if (!meta || !conversation) return false;
    const turn = s.views[conversation.id]?.turns.findLast((candidate) => candidate.requests.some((id) => meta.requestIds.includes(id)) || candidate.userMessage === meta.requestId);
    return turn?.end === "failed" && !!turn.limit;
  });
  const quotaFailure = nativeLimit || (error !== null && describeTurnError(error).limit === true);
  const wait = latest && meta && (isLive(meta.state) || quotaFailure) ? conversation?.quotaWait ?? null : null;
  const recovered = !!meta && quotaFailure && leadTransitions(meta.orchestratorSteps)
    .some((transition) => transition.startedAtMs >= (meta.endedAtMs ?? meta.startedAtMs));
  return { wait, recovered };
}

/**
 * The block's error as its requests have it (`cutOff`): a turn the thread says was cut by a quit
 * reads so, with Continue, only when its request was; else its request's own error.
 */
export function useBlockError(meta: BlockMeta | undefined, error: string | null): { error: string; resumable: boolean } | null {
  const requests = useBoard(
    useShallow((s) => {
      if (!meta || !s.board || !interruptedByQuit(error)) return [];
      const board = s.board;
      const own = meta.requestIds.flatMap((id) => board.requests[id] ?? []);
      return [latestRequest(board), ...own];
    }),
  );
  if (error === null) return null;
  const [latest = null, ...own] = requests;
  return cutOff(error, own.filter((request): request is UserRequest => request !== null), latest);
}

/** What a session's Try again sends, as the user's own message. */
const TRY_AGAIN = "Try again.";

/**
 * The block's error: one plain line, the error's own words under Details, and Try again where it
 * can be: a Chat answers again; a session's last turn gets "Try again." sent as the user's
 * message, the way the composer sends it. An overnight run's turn has none: the run goes on.
 */
export const BlockError: FC<{ meta: BlockMeta; live: boolean; quotaCovered?: boolean }> = ({ meta, live, quotaCovered = false }) => {
  const failed = useAuiState((s) => {
    const status = s.message.status;
    return status?.type === "incomplete" && status.reason === "error"
      ? String(status.error ?? "The reply failed.")
      : null;
  });
  const read = useBlockError(meta, failed);
  const latest = useAuiState((s) => s.message.isLast);
  const conversation = useViewConversation();
  const aui = useAui();
  const action = useAction();
  if (read === null || quotaCovered) return null;
  const { error, resumable } = read;
  const model = meta.texts.at(-1)?.model ?? meta.picked;
  const { summary, limit } = describeTurnError(error, model ? VENDOR_LABELS[model.provider] : undefined);
  // Brigadier quit while it ran: Continue picks the request up (UI audit H1).
  const interrupted = interruptedByQuit(error);
  const again = (() => {
    if (interrupted) {
      if (!resumable || live || !latest || !conversation || conversation.lifecycle === "archived" || action.busy) return undefined;
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
