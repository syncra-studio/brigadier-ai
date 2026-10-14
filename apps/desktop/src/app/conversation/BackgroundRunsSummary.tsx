import { Terminal } from "@openai/apps-sdk-ui/components/Icon";
import { useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { elapsedWords } from "@/app/conversation/liveStatus";
import { SummaryRow, SummarySection } from "@/components/assistant-ui/elements/summary-section";
import { Button } from "@/components/ui/button";
import { useNow } from "@/hooks/use-now";
import type { BackgroundRun } from "@/ipc/generated";
import { stopBackgroundRun } from "@/state/actions";
import { useBoard } from "@/state/board";

/**
 * The thread's long commands still running in the background, a row each: the command, how
 * long it has gone, and Stop. The thread hears a Stop and answers. Gone when none runs.
 */
export function BackgroundRunsSummary({ conversationId }: { conversationId: string }) {
  const runs = useBoard(useShallow((s) => s.board?.conversationId === conversationId
    ? Object.values(s.board.backgroundRuns)
        .filter((run) => run.state.type === "running")
        .toSorted((a, b) => a.startedAtMs - b.startedAtMs)
    : []));
  const now = useNow(runs.length ? 5_000 : null);
  if (!runs.length) return null;
  return (
    <SummarySection foldKey="running" title="Running" count={runs.length} aria-label="Running in the background">
      {runs.map((run) => <RunRow key={run.id} run={run} now={now} />)}
    </SummarySection>
  );
}

function RunRow({ run, now }: { run: BackgroundRun; now: number }) {
  const [stopping, setStopping] = useState(false);
  const [error, setError] = useState<string | null>(null);
  return (
    <SummaryRow
      data-slot="background-run"
      title={error ?? run.command}
      icon={<Terminal />}
      meta={
        <>
          <span data-slot="background-run-elapsed">{elapsedWords(now - run.startedAtMs)}</span>
          <Button
            variant="ghost"
            size="xs"
            disabled={stopping}
            aria-label={`Stop ${run.command}`}
            onClick={() => {
              setStopping(true);
              setError(null);
              stopBackgroundRun(run.conversationId, run.id).catch((reason: unknown) => {
                setStopping(false);
                setError(`Couldn't stop it: ${String(reason)}`);
              });
            }}
          >
            Stop
          </Button>
        </>
      }
    >
      <span className="font-mono text-xs">{run.command}</span>
    </SummaryRow>
  );
}
