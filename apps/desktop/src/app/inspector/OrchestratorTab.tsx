import { useVirtualizer } from "@tanstack/react-virtual";
import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { ContextChart } from "@/app/inspector/ContextChart";
import { Picker } from "@/app/inspector/providers/Picker";
import {
  type BreachRow,
  type ContextPoint,
  deriveLog,
  formatTokens,
  type Generation,
  GROUPS,
  groupOf,
  type InjectionRow,
  KIND_LABELS,
  sumOf,
} from "@/app/inspector/orchestratorLog";
import { RebirthLog } from "@/app/inspector/RebirthLog";
import { ThreadMetricsPanel } from "@/app/inspector/ThreadMetricsPanel";
import { Button } from "@/components/ui/button";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import type { RebirthThresholds } from "@/ipc/generated";
import { formatBytes, formatClock } from "@/lib/format";
import { tokenPx } from "@/lib/tokens";
import { cn } from "@/lib/utils";
import {
  loadEarlierOrchestratorLog,
  openOrchestratorLog,
  shownBeforeSettings,
} from "@/state/actions";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";

const NO_ENTRIES: never[] = [];

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * A session's orchestrator: every context injection against the CLI's reported context size,
 * so it is visible that its context grows only by messages and reports. It starts on the
 * session open before Settings (else the newest one); the picker shows another.
 */
export function OrchestratorTab() {
  const conversations = useApp((s) => s.conversations);
  // The sessions to choose from, newest first.
  const options = useMemo(
    () =>
      Object.values(conversations)
        .filter(
          (conversation) =>
            conversation.kind === "session" &&
            conversation.lifecycle !== "archived" &&
            conversation.sideOf === null,
        )
        .toSorted((a, b) => b.updatedAtMs - a.updatedAtMs)
        .map((conversation) => ({ value: conversation.id, label: conversation.title })),
    [conversations],
  );
  const [picked, setPicked] = useState<string | null>(() => {
    const before = shownBeforeSettings();
    return before.type === "conversation" ? before.id : null;
  });
  const sessionId =
    options.find((option) => option.value === picked)?.value ?? options[0]?.value ?? null;
  const [failure, setFailure] = useState<{ sessionId: string; message: string } | null>(null);

  // Follow the open session's log while this tab shows; stop following when it closes.
  useEffect(() => {
    if (sessionId === null) return undefined;
    openOrchestratorLog(sessionId).catch((cause: unknown) =>
      setFailure({ sessionId, message: errorText(cause) }),
    );
    return () => {
      void openOrchestratorLog(null);
    };
  }, [sessionId]);

  if (sessionId === null) {
    return (
      <p className="text-muted-foreground p-4 text-xs">
        Start a session to see its orchestrator.
      </p>
    );
  }
  const error = failure?.sessionId === sessionId ? failure.message : null;
  return (
    <>
      <div className="flex shrink-0 items-center gap-2 border-b px-3 py-2 text-xs">
        <span className="text-muted-foreground">Session</span>
        <Picker label="Session" value={sessionId} options={options} onChange={setPicked} />
      </div>
      <OrchestratorLogView sessionId={sessionId} error={error} />
    </>
  );
}

function OrchestratorLogView({ sessionId, error }: { sessionId: string; error: string | null }) {
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
          <p className="text-muted-foreground">
            Totals count from the oldest loaded entry.{" "}
            <Button
              size="xs"
              variant="ghost"
              disabled={log.loading}
              onClick={() => {
                setEarlierError(null);
                loadEarlierOrchestratorLog().catch((cause: unknown) =>
                  setEarlierError(errorText(cause)),
                );
              }}
            >
              Load earlier entries
            </Button>
          </p>
        )}
        {(error ?? earlierError) && (
          <p role="alert" className="text-destructive">
            {error ?? earlierError}
          </p>
        )}
      </div>
      <div className="text-muted-foreground flex shrink-0 items-center gap-2 border-b px-3 py-1.5 text-xs">
        <ToggleGroup
          type="single"
          size="sm"
          variant="outline"
          value={list}
          onValueChange={(value) => {
            if (value === "injections" || value === "rebirths") setList(value);
          }}
          aria-label="Orchestrator log"
        >
          <ToggleGroupItem value="injections" className="text-xs">
            Context injections · {derived.injections.length}
          </ToggleGroupItem>
          <ToggleGroupItem value="rebirths" className="text-xs">
            Rebirths · {derived.rebirths.length}
          </ToggleGroupItem>
        </ToggleGroup>
        {list === "injections" && <span className="flex-1 text-end">newest first, by CLI generation</span>}
      </div>
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

/** Contract breaches (the CLI compacted its context): errors, above everything else. */
function Breaches({ breaches }: { breaches: readonly BreachRow[] }) {
  if (breaches.length === 0) return null;
  return (
    <ul role="alert" className="border-destructive/40 bg-destructive/10 rounded-control flex flex-col gap-1 border px-2.5 py-2">
      {breaches.map((breach) => (
        <li key={breach.streamSeq} className="flex gap-2">
          <span className="text-destructive shrink-0 font-medium">Contract breach</span>
          <span className="min-w-0 flex-1 wrap-break-word">{breach.message}</span>
          <span className="text-muted-foreground shrink-0 tabular-nums">{formatClock(breach.atMs)}</span>
        </li>
      ))}
    </ul>
  );
}

function ContextMeter({
  point,
  injected,
  generation,
  thresholds,
}: {
  point: ContextPoint | null;
  injected: number;
  generation: number;
  thresholds: RebirthThresholds | null;
}) {
  const used = point?.usedTokens ?? null;
  const windowTokens = point?.windowTokens ?? null;
  const share = used !== null && windowTokens ? Math.min(100, (used / windowTokens) * 100) : null;
  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-baseline gap-2">
        <span className="flex-1 font-medium">
          Context{" "}
          <span className="text-muted-foreground font-normal">
            · CLI generation {generation}
          </span>
        </span>
        <span className="text-muted-foreground tabular-nums">
          {used === null
            ? "not reported yet"
            : `${formatTokens(used)}${windowTokens ? ` of ${formatTokens(windowTokens)}` : ""} tokens${
                share === null ? "" : ` · ${Math.round(share)}%`
              }`}
        </span>
      </div>
      <div
        role="meter"
        aria-label="Orchestrator context used"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={share === null ? undefined : Math.round(share)}
        className="bg-muted h-1.5 overflow-hidden rounded-full"
      >
        {share !== null && (
          <div
            className={cn("h-full", share >= 90 ? "bg-destructive" : "bg-primary")}
            style={{ width: `${share}%` }}
          />
        )}
      </div>
      <span className="text-muted-foreground tabular-nums">
        Brigadier injected about {formatTokens(injected)} tokens into this CLI (≈ 4 bytes per
        token)
        {thresholds &&
          ` · handoff prepared at ${formatTokens(thresholds.prepareTokens)}, reborn at ${formatTokens(thresholds.swapTokens)}`}
      </span>
    </div>
  );
}

const SWATCHES = Object.fromEntries(GROUPS.map((group) => [group.id, group.swatch]));

const InjectionRowView = memo(function InjectionRowView({
  row,
  taskNumber,
}: {
  row: InjectionRow;
  taskNumber: number | null;
}) {
  const { injection } = row;
  return (
    <div className="h-row-sm flex items-center gap-2 px-3 font-mono text-xs">
      <span className="text-muted-foreground shrink-0 tabular-nums">{formatClock(row.atMs)}</span>
      <span
        aria-hidden
        className={cn("size-2 shrink-0 rounded-xs", SWATCHES[groupOf(injection.kind)])}
      />
      <span className="text-foreground shrink-0">{KIND_LABELS[injection.kind]}</span>
      <span className="text-muted-foreground min-w-0 flex-1 truncate" title={injection.label}>
        {injection.label}
        {taskNumber !== null && ` · task-${taskNumber}`}
      </span>
      <span className="shrink-0 tabular-nums">{formatTokens(injection.tokensEstimate)} tok</span>
      <span className="text-muted-foreground w-16 shrink-0 text-end tabular-nums">
        {formatBytes(injection.bytes)}
      </span>
    </div>
  );
});

type ListItem =
  | { type: "injection"; row: InjectionRow }
  | { type: "generation"; generation: Generation; injections: number };

/** Newest first, like the Events tab, with a header over each CLI generation's injections. */
function listItems(injections: readonly InjectionRow[], generations: readonly Generation[]): ListItem[] {
  const items: ListItem[] = [];
  for (let index = generations.length - 1; index >= 0; index -= 1) {
    const generation = generations[index];
    if (!generation) continue;
    const to = generations[index + 1]?.from ?? injections.length;
    items.push({ type: "generation", generation, injections: to - generation.from });
    for (let row = to - 1; row >= generation.from; row -= 1) {
      const injection = injections[row];
      if (injection) items.push({ type: "injection", row: injection });
    }
  }
  return items;
}

function GenerationHeader({ generation, injections }: { generation: Generation; injections: number }) {
  const { rebirth } = generation;
  return (
    <div className="h-row-sm bg-muted/40 text-muted-foreground flex items-center gap-2 px-3 text-xs">
      <span className="text-foreground font-medium">Generation {generation.generation}</span>
      <span className="min-w-0 flex-1 truncate">
        {rebirth
          ? `reborn ${formatClock(rebirth.atMs)} at ${formatTokens(rebirth.record.atTokens)} tokens · briefing ${formatTokens(rebirth.record.briefingTokens)}`
          : "the first CLI session"}
      </span>
      <span className="shrink-0 tabular-nums">{injections} injections</span>
    </div>
  );
}

function InjectionList({
  injections,
  generations,
  loading,
}: {
  injections: readonly InjectionRow[];
  generations: readonly Generation[];
  loading: boolean;
}) {
  const density = useApp((s) => s.settings.density);
  const tasks = useBoard((s) => s.board?.tasks);
  const scrollRef = useRef<HTMLDivElement>(null);
  // A single generation needs no header.
  const items = useMemo(
    () =>
      generations.length > 1
        ? listItems(injections, generations)
        : injections.toReversed().map((row) => ({ type: "injection" as const, row })),
    [injections, generations],
  );
  const count = items.length;
  const at = (index: number) => items[index];

  // The app does not use React Compiler, so the virtualizer's unmemoizable API is fine here.
  // oxlint-disable-next-line react/incompatible-library
  const virtualizer = useVirtualizer({
    count,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => tokenPx("--spacing-row-sm"),
    getItemKey: (index) => {
      const item = at(index);
      if (!item) return index;
      return item.type === "injection" ? item.row.streamSeq : `generation:${item.generation.generation}`;
    },
    overscan: 10,
  });

  // Row height is a density token.
  useLayoutEffect(() => {
    virtualizer.measure();
  }, [density, virtualizer]);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div ref={scrollRef} data-selectable className="min-h-0 flex-1 overflow-y-auto">
        {injections.length === 0 ? (
          <p className="text-muted-foreground p-4 text-xs">
            {loading ? "Loading…" : "Nothing has entered the orchestrator's context yet."}
          </p>
        ) : (
          <div className="relative w-full" style={{ height: `${virtualizer.getTotalSize()}px` }}>
            {virtualizer.getVirtualItems().map((item) => {
              const entry = at(item.index);
              if (!entry) return null;
              const taskId = entry.type === "injection" ? entry.row.injection.taskId : null;
              return (
                <div
                  key={item.key}
                  className="absolute inset-x-0 top-0"
                  style={{ transform: `translateY(${item.start}px)` }}
                >
                  {entry.type === "injection" ? (
                    <InjectionRowView
                      row={entry.row}
                      taskNumber={taskId ? (tasks?.[taskId]?.number ?? null) : null}
                    />
                  ) : (
                    <GenerationHeader generation={entry.generation} injections={entry.injections} />
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
