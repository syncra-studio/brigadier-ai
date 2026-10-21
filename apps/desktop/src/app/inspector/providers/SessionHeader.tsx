import { PROVIDER_LABELS, QuotaWindows, STATE_VARIANTS } from "@/app/inspector/providers/shared";
import type { TranscriptStats } from "@/components/conversation";
import { Badge, Button } from "@/components/controls";
import { ArrowLeft } from "@/components/icons";
import type { RawSession } from "@/ipc/generated";
import {
  closeRawSession,
  forkRawSession,
  resumeRawSession,
  selectRawSession,
  stopRawSession,
} from "@/state/actions";

export function SessionHeader({
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
