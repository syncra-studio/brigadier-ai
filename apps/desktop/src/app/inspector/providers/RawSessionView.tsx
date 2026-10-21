import { useMemo, useState } from "react";

import { ApprovalsTray } from "@/app/inspector/providers/ApprovalsTray";
import { RawSessionComposer } from "@/app/inspector/providers/RawSessionComposer";
import { SessionHeader } from "@/app/inspector/providers/SessionHeader";
import { TranscriptList } from "@/app/inspector/providers/TranscriptList";
import { foldTranscript } from "@/components/conversation";
import { Button } from "@/components/controls";
import { ArrowLeft } from "@/components/icons";
import { interruptRawSession, selectRawSession, sendRawSession } from "@/state/actions";
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
        <ApprovalsTray sessionId={id} requests={folded.pending} act={act} />
      )}
      {live && (
        <RawSessionComposer
          disabled={!running}
          turnActive={folded.stats.turnActive}
          onSend={(text, steer) => act(() => sendRawSession(id, text, steer))}
          onInterrupt={() => act(() => interruptRawSession(id))}
        />
      )}
    </div>
  );
}
