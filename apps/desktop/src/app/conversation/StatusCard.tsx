import { createContext, type FC, type ReactNode, useEffect, useState } from "react";

import type { ContextUsage, ConversationStatus, QuotaWindow } from "@/ipc/generated";

import { usageTone } from "@/lib/quota";

import { getConversationStatus } from "@/state/actions";

import { useBoard } from "@/state/board";

import { useApp } from "@/state/store";

import { ComposerRailItem } from "@/components/conversation/elements/composer-rail";
import { UsageBar } from "@/components/shared/usage-bar";

/** Whether the open conversation shows the `/status` card above its composer. */
export const StatusCardContext = createContext<{ open: boolean; setOpen: (open: boolean) => void }>({
  open: false,
  setOpen: () => {},
});

/** "258K": a window size in thousands. */
function thousands(tokens: number): string {
  return `${Math.round(tokens / 1000)}K`;
}

function contextText(context: ContextUsage): { value: string; detail: string | null } {
  const used = context.usedTokens.toLocaleString("en-US");
  if (!context.windowTokens || context.windowTokens <= 0) return { value: `${used} used`, detail: null };
  const left = Math.max(0, Math.round(100 - (context.usedTokens / context.windowTokens) * 100));
  return { value: `${left}% left`, detail: `(${used} used / ${thousands(context.windowTokens)})` };
}

/** "5h", "7d": a usage window by its length, else the provider's own name for it. */
export function windowName(window: QuotaWindow): string {
  const minutes = window.windowMinutes;
  if (minutes && minutes % 1440 === 0) return `${minutes / 1440}d`;
  if (minutes && minutes % 60 === 0) return `${minutes / 60}h`;
  return window.label;
}

/** When a window resets: its time for a window shorter than a day, else its date. */
export function resetsAt(ms: number, window: QuotaWindow): string {
  const date = new Date(ms);
  if (window.windowMinutes !== null && window.windowMinutes < 1440) {
    return date.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  }
  return date.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

const Row: FC<{ label: string; children: ReactNode }> = ({ label, children }) => (
  <>
    <dt className="text-muted-foreground">{label}</dt>
    <dd className="flex min-w-0 items-center gap-2">{children}</dd>
  </>
);

/** A usage window's bar: the share left, filled; the share used, faint. */
/** What is left of a window, coloured by how much is used (from 80% and from 90%). */
const LimitBar: FC<{ left: number }> = ({ left }) => (
  <UsageBar
    value={left}
    fill={usageTone(100 - left, { normal: { fill: "bg-foreground" } }).fill}
    size="md"
    className="w-28"
  />
);

/**
 * The `/status` card, attached above the composer: the model's CLI session, how much context
 * is left, and how much of each usage window is left and when it resets.
 */
export const StatusCard: FC<{ conversationId: string; onClose: () => void }> = ({
  conversationId,
  onClose,
}) => {
  // Read once per conversation the card opens on.
  const [read, setRead] = useState<{
    conversationId: string;
    status: ConversationStatus | null;
    error: string | null;
  } | null>(null);
  const context = useBoard((s) =>
    s.board?.conversationId === conversationId ? s.board.context : null,
  );
  // A session's orchestrator is reborn with a fresh context; its meter is in the Inspector.
  const session = useApp((s) => s.conversations[conversationId]?.kind === "session");
  useEffect(() => {
    let current = true;
    getConversationStatus(conversationId).then(
      (status) => current && setRead({ conversationId, status, error: null }),
      (cause: unknown) =>
        current &&
        setRead({
          conversationId,
          status: null,
          error: cause instanceof Error ? cause.message : String(cause),
        }),
    );
    return () => {
      current = false;
    };
  }, [conversationId]);
  const status = read?.conversationId === conversationId ? read.status : null;
  const error = read?.conversationId === conversationId ? read.error : null;

  const shown = context ? contextText(context) : null;
  return (
    <ComposerRailItem label="Status">
      <div data-slot="status-card" className="px-3 pt-2 pb-3">
        <header className="flex items-center justify-between text-sm">
          <h2 className="text-muted-foreground">Status</h2>
          <button
            type="button"
            onClick={onClose}
            className="text-muted-foreground hover:text-foreground transition-colors"
          >
            Close
          </button>
        </header>
        {error ? (
          <p role="alert" className="text-destructive mt-2 text-xs">
            {error}
          </p>
        ) : (
          <dl className="mt-2 grid grid-cols-[max-content_minmax(0,1fr)] gap-x-4 gap-y-1 font-mono text-xs">
            <Row label="Session/Thread:">
              <span className="truncate">{status ? (status.nativeId ?? "not started yet") : "…"}</span>
            </Row>
            {!session && (
              <Row label="Context:">
                {shown ? (
                  <span className="truncate">
                    {shown.value}
                    {shown.detail && <span className="text-muted-foreground"> {shown.detail}</span>}
                  </span>
                ) : (
                  <span className="text-muted-foreground">no usage yet</span>
                )}
              </Row>
            )}
            {status?.quota?.windows.map((window) => {
              const left = Math.max(0, Math.min(100, Math.round(100 - window.usedPercent)));
              return (
                <Row key={window.id} label={`${windowName(window)} limit:`}>
                  <LimitBar left={left} />
                  <span className="truncate">
                    <span className={usageTone(100 - left, { normal: { text: "" } }).text}>{left}% left</span>
                    {window.resetsAtMs !== null && (
                      <span className="text-muted-foreground"> (resets {resetsAt(window.resetsAtMs, window)})</span>
                    )}
                  </span>
                </Row>
              );
            })}
          </dl>
        )}
      </div>
    </ComposerRailItem>
  );
};
