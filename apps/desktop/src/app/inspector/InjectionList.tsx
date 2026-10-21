import { useVirtualizer } from "@tanstack/react-virtual";
import { useMemo, useRef } from "react";

import { GenerationHeader } from "@/app/inspector/GenerationHeader";
import { InjectionRowView } from "@/app/inspector/InjectionRowView";
import type { Generation, InjectionRow } from "@/app/inspector/orchestratorLog";
import { tokenPx } from "@/lib/tokens";
import { useBoard } from "@/state/board";

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

export function InjectionList({
  injections,
  generations,
  loading,
}: {
  injections: readonly InjectionRow[];
  generations: readonly Generation[];
  loading: boolean;
}) {
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
