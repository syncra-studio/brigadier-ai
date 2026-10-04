import { ArrowLeft } from "@openai/apps-sdk-ui/components/Icon";
import { useLayoutEffect, useMemo, useRef, useState } from "react";

import {
  PROVIDER_LABELS,
  QuotaWindows,
  STATE_VARIANTS,
} from "@/app/inspector/providers/shared";
import {
  foldTranscript,
  type TranscriptItem,
  type TranscriptStats,
} from "@/components/transcript/transcript";
import { TranscriptRow } from "@/components/transcript/TranscriptRow";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type { ApprovalRequest, RawSession } from "@/ipc/generated";
import {
  answerApproval,
  closeRawSession,
  forkRawSession,
  interruptRawSession,
  loadEarlierRawEntries,
  resumeRawSession,
  selectRawSession,
  sendRawSession,
  stopRawSession,
} from "@/state/actions";
import { useApp } from "@/state/store";

const EMPTY: never[] = [];

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** One raw session: its controls, live figures, transcript and composer. */
export function RawSessionView({ id }: { id: string }) {
  const session = useApp((s) => s.providers.view?.sessions.find((entry) => entry.id === id));
  const transcript = useApp((s) => s.providers.transcripts[id]);
  const entries = transcript?.entries ?? EMPTY;
  const folded = useMemo(() => foldTranscript(entries), [entries]);
  const [error, setError] = useState<string | null>(null);

  const act = (action: () => Promise<void>) => {
    setError(null);
    action().catch((err: unknown) => setError(errorText(err)));
  };

  if (!session) {
    return (
      <div className="p-4 text-xs">
        <Button size="xs" variant="ghost" onClick={() => selectRawSession(null)}>
          <ArrowLeft />
          Sessions
        </Button>
        <p className="text-muted-foreground mt-2">This session is not known to the daemon.</p>
      </div>
    );
  }

  const live = session.source.type === "live";
  const running = session.state === "running";
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <SessionHeader session={session} stats={folded.stats} act={act} />
      {error && <p className="text-destructive border-b px-3 py-1.5 text-xs">{error}</p>}
      <TranscriptList
        id={id}
        items={folded.items}
        hasMore={transcript?.hasMore ?? false}
        loading={transcript?.loading ?? true}
      />
      {folded.pending.length > 0 && running && (
        <div className="flex max-h-60 shrink-0 flex-col gap-2 overflow-y-auto border-t p-2">
          {folded.pending.map((request) => (
            <ApprovalCard key={request.id} sessionId={id} request={request} act={act} />
          ))}
        </div>
      )}
      {live && (
        <Composer
          disabled={!running}
          turnActive={folded.stats.turnActive}
          onSend={(text, steer) => act(() => sendRawSession(id, text, steer))}
          onInterrupt={() => act(() => interruptRawSession(id))}
        />
      )}
    </div>
  );
}

function SessionHeader({
  session,
  stats,
  act,
}: {
  session: RawSession;
  stats: TranscriptStats;
  act: (action: () => Promise<void>) => void;
}) {
  const live = session.source.type === "live";
  const { state } = session;
  return (
    <div className="flex shrink-0 flex-col gap-1.5 border-b px-3 py-2 text-xs">
      <div className="flex items-center gap-2">
        <Button
          size="icon-sm"
          variant="ghost"
          aria-label="Back to sessions"
          onClick={() => selectRawSession(null)}
        >
          <ArrowLeft />
        </Button>
        <span className="text-sm font-medium">{PROVIDER_LABELS[session.provider]}</span>
        <Badge variant={STATE_VARIANTS[state]}>{state}</Badge>
        {session.source.type !== "live" && (
          <Badge variant="outline">{session.source.type}</Badge>
        )}
        {stats.turnActive && state === "running" && <Badge variant="warning">turn running</Badge>}
        <span className="flex-1" />
        {live && state === "running" && (
          <Button size="xs" variant="outline" onClick={() => act(() => stopRawSession(session.id))}>
            Stop
          </Button>
        )}
        {live && state === "stopped" && session.nativeId && (
          <Button size="xs" variant="outline" onClick={() => act(() => resumeRawSession(session.id))}>
            Resume
          </Button>
        )}
        {live && (state === "running" || state === "stopped") && session.nativeId && (
          <Button size="xs" variant="outline" onClick={() => act(() => forkRawSession(session.id))}>
            Fork
          </Button>
        )}
        {live && state !== "closing" && state !== "closed" && (
          <Button
            size="xs"
            variant="outline"
            title="End the CLI and remove every file its session created"
            onClick={() => act(() => closeRawSession(session.id))}
          >
            Close
          </Button>
        )}
      </div>
      <div data-selectable className="text-muted-foreground flex flex-wrap gap-x-3 gap-y-0.5">
        {session.cwd && <span className="font-mono">{session.cwd}</span>}
        <span>
          {stats.model ?? session.model ?? "default model"}
          {session.effort && ` · ${session.effort}`}
        </span>
        {live && (
          <span>
            {session.access.type === "workspace" ? "workspace" : session.access.type} ·{" "}
            {session.approvals === "delegated" ? "policy + me" : "decline all"}
          </span>
        )}
        {session.nativeId && <span className="font-mono">{session.nativeId}</span>}
        {stats.context && (
          <span className="tabular-nums">
            context {stats.context.usedTokens.toLocaleString()}
            {stats.context.windowTokens !== null &&
              ` / ${stats.context.windowTokens.toLocaleString()}`}
          </span>
        )}
        {stats.usage && (
          <span className="tabular-nums">
            {stats.usage.inputTokens.toLocaleString()} in ·{" "}
            {stats.usage.cachedInputTokens.toLocaleString()} cached ·{" "}
            {stats.usage.outputTokens.toLocaleString()} out
            {stats.usage.costUsd !== null && ` · $${stats.usage.costUsd.toFixed(4)}`}
          </span>
        )}
        {session.recording && <span className="font-mono">recording → {session.recording}</span>}
      </div>
      {session.error && <p className="text-destructive">{session.error}</p>}
      {stats.quota && <QuotaWindows quota={stats.quota} />}
    </div>
  );
}

function TranscriptList({
  id,
  items,
  hasMore,
  loading,
}: {
  id: string;
  items: readonly TranscriptItem[];
  hasMore: boolean;
  loading: boolean;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);

  // Follow new output while the view is scrolled to the bottom (items only change with it).
  useLayoutEffect(() => {
    const element = scrollRef.current;
    if (element && pinned.current) element.scrollTop = element.scrollHeight;
  });

  return (
    <div
      ref={scrollRef}
      data-selectable
      className="flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto p-3 text-xs"
      onScroll={(event) => {
        const element = event.currentTarget;
        pinned.current = element.scrollHeight - element.scrollTop - element.clientHeight < 24;
      }}
    >
      {hasMore && (
        <Button
          size="xs"
          variant="ghost"
          className="self-center"
          disabled={loading}
          onClick={() => void loadEarlierRawEntries(id)}
        >
          Load earlier events
        </Button>
      )}
      {items.length === 0 && (
        <p className="text-muted-foreground">{loading ? "Loading…" : "No events yet."}</p>
      )}
      {items.map((item) => (
        <TranscriptRow key={item.key} item={item} />
      ))}
    </div>
  );
}

function ApprovalCard({
  sessionId,
  request,
  act,
}: {
  sessionId: string;
  request: ApprovalRequest;
  act: (action: () => Promise<void>) => void;
}) {
  const answer = (allow: boolean) =>
    act(() =>
      answerApproval(
        sessionId,
        request.id,
        allow ? { type: "allow" } : { type: "deny", message: "Denied by the user." },
      ),
    );
  return (
    <div className="border-warning/50 bg-warning/10 flex flex-col gap-1.5 rounded-lg border p-2.5 text-xs">
      <div className="flex items-center gap-2">
        <Badge variant="warning">{request.kind}</Badge>
        <span className="font-mono">{request.tool}</span>
        {request.escalation && <Badge variant="destructive">outside the sandbox</Badge>}
      </div>
      {request.command && <pre className="font-mono whitespace-pre-wrap">{request.command}</pre>}
      {request.paths.length > 0 && (
        <p className="font-mono">{request.paths.join("\n")}</p>
      )}
      {request.reason && <p className="text-muted-foreground">{request.reason}</p>}
      <div className="flex justify-end gap-2">
        <Button size="xs" variant="outline" onClick={() => answer(false)}>
          Deny
        </Button>
        <Button size="xs" onClick={() => answer(true)}>
          Allow
        </Button>
      </div>
    </div>
  );
}

function Composer({
  disabled,
  turnActive,
  onSend,
  onInterrupt,
}: {
  disabled: boolean;
  turnActive: boolean;
  onSend: (text: string, steer: boolean) => void;
  onInterrupt: () => void;
}) {
  const [text, setText] = useState("");
  const send = (steer: boolean) => {
    const trimmed = text.trim();
    if (!trimmed) return;
    onSend(trimmed, steer);
    setText("");
  };
  return (
    <div className="flex shrink-0 flex-col gap-2 border-t p-2">
      <textarea
        aria-label="Message"
        value={text}
        disabled={disabled}
        rows={3}
        placeholder={disabled ? "The session is not running." : "Message (⌘↩ to send, ⇧⌘↩ to steer)"}
        onChange={(event) => setText(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
            event.preventDefault();
            send(event.shiftKey);
          }
        }}
        className="bg-muted/60 focus-visible:bg-background rounded-control focus-visible:ring-ring/50 w-full resize-none border border-transparent p-2 text-xs outline-none focus-visible:ring-1 disabled:opacity-50"
      />
      <div className="flex items-center gap-2">
        <span className="text-muted-foreground flex-1 text-xs">
          Steer delivers into the running turn, or starts one.
        </span>
        <Button size="xs" variant="outline" disabled={disabled || !turnActive} onClick={onInterrupt}>
          Interrupt
        </Button>
        <Button size="xs" variant="outline" disabled={disabled || !text.trim()} onClick={() => send(true)}>
          Steer
        </Button>
        <Button size="xs" disabled={disabled || !text.trim()} onClick={() => send(false)}>
          Send
        </Button>
      </div>
    </div>
  );
}
