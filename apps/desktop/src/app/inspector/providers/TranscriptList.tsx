import { useLayoutEffect, useRef } from "react";

import { type TranscriptItem, TranscriptRow } from "@/components/conversation";
import { Button } from "@/components/controls";
import { loadEarlierRawEntries } from "@/state/actions";

export function TranscriptList({
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
