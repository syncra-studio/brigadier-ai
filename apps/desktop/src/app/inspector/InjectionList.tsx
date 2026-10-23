import { useVirtualizer } from "@tanstack/react-virtual";
import { useMemo, useRef } from "react";

import { InjectionRowView } from "@/app/inspector/InjectionRowView";
import type { InjectionRow } from "@/app/inspector/orchestratorLog";
import { tokenPx } from "@/lib/tokens";
import { useBoard } from "@/state/board";

/** Newest first, like the Events tab. */
export function InjectionList({
  injections,
  loading,
}: {
  injections: readonly InjectionRow[];
  loading: boolean;
}) {
  const tasks = useBoard((s) => s.board?.tasks);
  const scrollRef = useRef<HTMLDivElement>(null);
  const items = useMemo(() => injections.toReversed(), [injections]);
  const count = items.length;
  const at = (index: number) => items[index];

  // The app does not use React Compiler, so the virtualizer's unmemoizable API is fine here.
  // oxlint-disable-next-line react/incompatible-library
  const virtualizer = useVirtualizer({
    count,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => tokenPx("--spacing-row-sm"),
    getItemKey: (index) => at(index)?.streamSeq ?? index,
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
              const row = at(item.index);
              if (!row) return null;
              const taskId = row.injection.taskId;
              return (
                <div
                  key={item.key}
                  className="absolute inset-x-0 top-0"
                  style={{ transform: `translateY(${item.start}px)` }}
                >
                  <InjectionRowView
                    row={row}
                    taskNumber={taskId ? (tasks?.[taskId]?.number ?? null) : null}
                  />
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
