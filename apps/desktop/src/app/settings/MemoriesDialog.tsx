import { Brain } from "@openai/apps-sdk-ui/components/Icon";
import { useState } from "react";

import { ErrorLine, errorText } from "@/app/dialogs/fields";
import { SettingsButton } from "@/app/settings/parts";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { useNow } from "@/hooks/use-now";
import type { Node, Origin } from "@/ipc/generated";
import { formatAgo } from "@/lib/format";
import { forgetMemory } from "@/state/brain";
import { useApp } from "@/state/store";

const ORIGINS: Record<Origin, string> = {
  index: "the code index",
  skeleton: "a skeleton pass",
  enrichment: "an enrichment job",
  report: "a worker's report",
  orchestrator: "a session",
  user: "you",
};

/** Where a memory was learned: the conversation's title while it exists, else its origin. */
function Source({ memory }: { memory: Node }) {
  const { sessionId, origin } = memory.provenance;
  const conversation = useApp((s) => (sessionId ? s.conversations[sessionId] : undefined));
  if (conversation) {
    return (
      <>
        from the {conversation.kind === "chat" ? "chat" : "session"} “{conversation.title}”
      </>
    );
  }
  return <>from {ORIGINS[origin]}</>;
}

/** The Personal Brain's memories, newest first, each one forgotten at once on Forget. */
export function MemoriesDialog({
  open,
  onOpenChange,
  memories,
  onForgotten,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  memories: readonly Node[];
  onForgotten: (id: string) => void;
}) {
  const [error, setError] = useState<string | null>(null);
  const [forgetting, setForgetting] = useState<string | null>(null);
  const now = useNow(60_000);

  const forget = (id: string) => {
    setForgetting(id);
    setError(null);
    forgetMemory(id)
      .then(() => onForgotten(id))
      .catch((cause: unknown) => setError(errorText(cause)))
      .finally(() => setForgetting(null));
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex max-h-9/10 max-w-lg flex-col">
        <DialogHeader>
          <DialogTitle>Memories</DialogTitle>
          <DialogDescription>
            What Brigadier remembers about you. Every Chat and session uses them.
          </DialogDescription>
        </DialogHeader>
        <ErrorLine error={error} />
        {memories.length === 0 ? (
          <div className="text-muted-foreground flex items-center gap-3 py-2 text-label">
            <Brain aria-hidden className="size-icon-md shrink-0" />
            Nothing yet. Tell a Chat what you prefer (“remember that I use pnpm”) and it shows up
            here.
          </div>
        ) : (
          <ul className="divide-divider -mx-1 min-h-0 flex-1 divide-y overflow-y-auto px-1">
            {memories.map((memory) => (
              <li key={memory.id} className="flex items-center justify-between gap-4 py-2.5">
                <div className="flex min-w-0 flex-1 flex-col gap-0.5">
                  <span className="text-label font-medium wrap-break-word">{memory.title}</span>
                  {memory.body && (
                    <span className="text-foreground/65 line-clamp-2 text-xs">{memory.body}</span>
                  )}
                  <span className="text-foreground/65 text-xs">
                    {formatAgo(memory.updatedAtMs, now)} · <Source memory={memory} />
                  </span>
                </div>
                <SettingsButton
                  destructive
                  aria-label={`Forget “${memory.title}”`}
                  disabled={forgetting === memory.id}
                  onClick={() => forget(memory.id)}
                >
                  Forget
                </SettingsButton>
              </li>
            ))}
          </ul>
        )}
      </DialogContent>
    </Dialog>
  );
}
