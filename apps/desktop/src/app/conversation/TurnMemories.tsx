import { type FC, useMemo } from "react";

import { MemoryChips } from "@/components/conversation";
import type { MemoryChange } from "@/ipc/generated";
import { useBoard } from "@/state/board";
import { forgetMemory } from "@/state/brain";
import { toast } from "@/state/toasts";

const NONE: MemoryChange[] = [];

/**
 * A Chat turn's Memory chips: what its model saved to the Personal Brain while answering these
 * requests. Removing a chip forgets the memory; the chip goes once the daemon confirms it.
 */
export const TurnMemories: FC<{ requestIds: readonly string[] }> = ({ requestIds }) => {
  const memories = useBoard((s) => s.board?.memories ?? NONE);
  const chips = useMemo(
    () =>
      memories
        .filter(
          (memory) =>
            !memory.forgotten && memory.requestId !== null && requestIds.includes(memory.requestId),
        )
        .map((memory) => ({ id: memory.nodeId, text: memory.text, change: "added" as const })),
    [memories, requestIds],
  );
  if (chips.length === 0) return null;
  return (
    <MemoryChips
      chips={chips}
      onForget={(nodeId) =>
        void forgetMemory(nodeId).catch((cause: unknown) =>
          toast(cause instanceof Error ? cause.message : String(cause), { tone: "error" }),
        )
      }
    />
  );
};
