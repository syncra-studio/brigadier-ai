import { type FC, type ReactNode, useMemo, useState } from "react";

import { newestFailedError, withoutTurnError } from "@/app/conversation/turnError";
import { useAction } from "@/app/conversation/useAction";
import { ComposerRailItem } from "@/components/conversation";
import { Button, TooltipIconButton } from "@/components/controls";
import { ExclamationMarkCircle, Info, Warning, X } from "@/components/icons";
import type { Notice } from "@/ipc/generated";
import { dismissNotice, loadConversation } from "@/state/actions";
import { useBoard } from "@/state/board";
import { useThreadView } from "@/state/cliThread";
import { useApp } from "@/state/store";

const NO_NOTICES: Notice[] = [];

/**
 * What the user must act on, or what stopped the turn, in one tray joined to the composer's top
 * (#506): messages that couldn't load (Retry), notices (environment problems, failed work) and a
 * failed run, one row each, newest notice last. Limits and switches are dividers in the thread.
 */
export const Notices: FC<{ conversationId: string }> = ({ conversationId }) => {
  const notices = useBoard((s) => (s.board?.conversationId === conversationId ? s.board.notices : NO_NOTICES));
  // A run that failed a request is said by that request in the thread, once.
  const runError = useBoard((s) =>
    s.board?.conversationId === conversationId && s.board.run === "failed" && !s.board.runRequest
      ? (s.board.runError ?? "The model stopped with an error.")
      : null,
  );
  // What the newest failed turn already says in the thread is never said again here.
  const turns = useThreadView(conversationId)?.turns;
  const turnError = useMemo(() => (turns ? newestFailedError(turns) : null), [turns]);
  // Gone at once; the daemon keeps it dismissed through every read and redraw (D51).
  const [dismissed, setDismissed] = useState<ReadonlySet<number>>(new Set());
  const dismiss = useAction();
  const shown = useMemo(
    () => withoutTurnError(notices, turnError).filter((notice) => !dismissed.has(notice.atMs)).slice(-3),
    [notices, turnError, dismissed],
  );
  const failure = runError !== null && runError.trim() !== turnError?.trim() ? runError : null;
  const [failureDismissed, setFailureDismissed] = useState<string | null>(null);
  const failed = failure !== null && failure !== failureDismissed;
  // Its messages could not be loaded: said once, with Retry.
  const loadError = useApp((s) => s.threads[conversationId]?.loadError ?? null);
  const reload = useAction();
  if (shown.length === 0 && !failed && loadError === null) return null;
  return (
    <ComposerRailItem label="Notices">
      <ul className="divide-border flex flex-col divide-y">
        {loadError !== null && (
          <NoticeRow
            slot="load-error"
            role="alert"
            title={loadError}
            icon={<Warning aria-hidden className="text-warning" />}
            action={
              <Button
                variant="ghost"
                size="xs"
                disabled={reload.busy}
                className="text-link hover:text-link/80 -my-0.5"
                onClick={() => reload.run(() => loadConversation(conversationId))}
              >
                Retry
              </Button>
            }
          >
            Couldn’t load messages
          </NoticeRow>
        )}
        {shown.map((notice) => (
          <NoticeRow
            key={`${notice.atMs}:${notice.text}`}
            level={notice.level}
            icon={
              notice.level === "warning" ? (
                <Warning aria-hidden className="text-warning" />
              ) : (
                <Info aria-hidden className="text-muted-foreground" />
              )
            }
            action={
              <Dismiss
                label="Dismiss notice"
                onClick={() => {
                  setDismissed((current) => new Set(current).add(notice.atMs));
                  dismiss.run(() => dismissNotice(conversationId, notice.atMs));
                }}
              />
            }
          >
            {notice.text}
          </NoticeRow>
        ))}
        {failed && (
          <NoticeRow
            slot="run-failure"
            role="status"
            icon={<ExclamationMarkCircle aria-hidden className="text-destructive" />}
            action={<Dismiss label="Dismiss" onClick={() => setFailureDismissed(failure)} />}
          >
            {failure}
          </NoticeRow>
        )}
      </ul>
    </ComposerRailItem>
  );
};

/** One row of the tray: a tinted icon, the words, and its one action (Retry or ×). */
const NoticeRow: FC<{
  slot?: string;
  role?: "alert" | "status";
  level?: Notice["level"];
  title?: string;
  icon: ReactNode;
  action: ReactNode;
  children: ReactNode;
}> = ({ slot, role, level, title, icon, action, children }) => (
  <li
    data-slot={slot}
    data-level={level}
    role={role}
    title={title}
    className="text-foreground text-label flex items-start gap-2.5 py-2 pr-2.5 pl-3.5 [&>svg]:size-icon-sm [&>svg]:mt-0.5 [&>svg]:shrink-0"
  >
    {icon}
    <span data-slot="notice-text" className="min-w-0 flex-1 whitespace-pre-wrap">
      {children}
    </span>
    {action}
  </li>
);

const Dismiss: FC<{ label: string; onClick: () => void }> = ({ label, onClick }) => (
  <TooltipIconButton tooltip={label} size="icon-xs" className="text-muted-foreground hover:text-foreground shrink-0" onClick={onClick}>
    <X />
  </TooltipIconButton>
);
