import type { FC } from "react";

import { folderOf } from "@/app/conversation/forks";
import { useViewConversation } from "@/app/conversation/viewContext";
import { ROW } from "@/components/conversation";
import { BranchAlt } from "@/components/icons";
import { openConversation } from "@/state/actions";
import { useApp } from "@/state/store";

/**
 * One quiet line where a fork starts, under the answer it was made from: "Forked from ‹source› ·
 * works in the same folder" (or a new worktree); the source's name opens it (D211).
 */
export const ContinuedFrom: FC<{ forkId: string | null }> = ({ forkId }) => {
  const fork = useViewConversation();
  const origin = fork?.forkedFrom ?? null;
  const source = useApp((s) => (origin ? s.conversations[origin.conversationId] : undefined));
  if (!fork || !origin || !forkId || origin.messageId !== forkId) return null;
  const setup = fork.setup?.type === "session" ? fork.setup : null;
  const sameFolder = setup?.environment.type === "localCheckout" || (source !== undefined && folderOf(setup) !== null && folderOf(setup) === folderOf(source.setup));
  const where = !setup ? null : sameFolder ? "works in the same folder" : "works in a new worktree";
  return (
    <div data-slot="forked-from" className={ROW}>
      <BranchAlt className="size-icon-sm" />
      <span className="min-w-0 truncate">
        Forked from{" "}
        {source ? (
          <button type="button" className="text-foreground hover:underline focus-visible:underline" onClick={() => openConversation(source.id)}>
            {source.title}
          </button>
        ) : (
          `another ${fork.kind}`
        )}
        {where && ` · ${where}`}
      </span>
    </div>
  );
};
