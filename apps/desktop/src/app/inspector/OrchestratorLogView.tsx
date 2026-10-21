import { useMemo, useState } from "react";

import { Breaches } from "@/app/inspector/Breaches";
import { ContextChart } from "@/app/inspector/ContextChart";
import { ContextMeter } from "@/app/inspector/ContextMeter";
import { InjectionList } from "@/app/inspector/InjectionList";
import { ListSwitchBar } from "@/app/inspector/ListSwitchBar";
import { LoadEarlierLine } from "@/app/inspector/LoadEarlierLine";
import { deriveLog, sumOf } from "@/app/inspector/orchestratorLog";
import { RebirthLog } from "@/app/inspector/RebirthLog";
import { ThreadMetricsPanel } from "@/app/inspector/ThreadMetricsPanel";
import { loadEarlierOrchestratorLog } from "@/state/actions";
import { useBoard } from "@/state/board";

const NO_ENTRIES: never[] = [];

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function OrchestratorLogView({ sessionId, error }: { sessionId: string; error: string | null }) {
  const log = useBoard((s) =>
    s.orchestrator?.conversationId === sessionId ? s.orchestrator : null,
  );
  const entries = log?.entries ?? NO_ENTRIES;
  const derived = useMemo(() => deriveLog(entries), [entries]);
  const thresholds = log?.thresholds ?? null;
  const [earlierError, setEarlierError] = useState<string | null>(null);
  const [list, setList] = useState<"injections" | "rebirths">("injections");
  const generation = derived.generations.at(-1)?.generation ?? 0;

  return (
    <>
      <div className="flex shrink-0 flex-col gap-3 border-b px-3 py-2.5 text-xs">
        <Breaches breaches={derived.breaches} />
        <ContextMeter
          point={derived.latestContext}
          injected={sumOf(derived.current)}
          generation={generation}
          thresholds={thresholds}
        />
        <ContextChart log={derived} thresholds={thresholds} />
        <ThreadMetricsPanel sessionId={sessionId} />
        {log?.hasMore && (
          <LoadEarlierLine
            loading={log.loading}
            onLoad={() => {
              setEarlierError(null);
              loadEarlierOrchestratorLog().catch((cause: unknown) =>
                setEarlierError(errorText(cause)),
              );
            }}
          />
        )}
        {(error ?? earlierError) && (
          <p role="alert" className="text-destructive">
            {error ?? earlierError}
          </p>
        )}
      </div>
      <ListSwitchBar
        list={list}
        onListChange={setList}
        injections={derived.injections.length}
        rebirths={derived.rebirths.length}
      />
      {list === "injections" ? (
        <InjectionList
          injections={derived.injections}
          generations={derived.generations}
          loading={log?.loading ?? true}
        />
      ) : (
        <RebirthLog rebirths={derived.rebirths} />
      )}
    </>
  );
}
