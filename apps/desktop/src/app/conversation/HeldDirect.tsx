import { ComposerRailItem, WorkerGlyph } from "@/components/conversation";

import { restoreDraft } from "@/app/conversation/composerDraft";
import { useWorkerName } from "@/app/conversation/WorkerChip";
import { Button } from "@/components/controls";
import { setDirectTo } from "@/state/directTo";
import { dropHeldDirect, type HeldDirect, heldDraft, useHeldDirectOf } from "@/state/heldDirect";
import { toast } from "@/state/toasts";

/**
 * The composer rail's direct messages a worker didn't take, kept whole (see state/heldDirect):
 * Restore puts one back into the empty composer with its To:, Discard lets it go.
 */
export function HeldDirectMessages({ conversationId }: { conversationId: string }) {
  const held = useHeldDirectOf(conversationId);
  return held.map((entry) => (
    <ComposerRailItem key={entry.scope} label="Message not sent">
      <HeldRow entry={entry} />
    </ComposerRailItem>
  ));
}

function HeldRow({ entry }: { entry: HeldDirect }) {
  const name = useWorkerName(entry.taskId);
  const restore = () => {
    const draft = heldDraft(entry.scope);
    if (!draft) {
      dropHeldDirect(entry.scope);
      return;
    }
    // Never mixed with what the composer holds now.
    if (!restoreDraft(entry.conversationId, draft)) {
      toast("Clear the composer to restore the message", { tone: "neutral" });
      return;
    }
    setDirectTo(entry.conversationId, entry.taskId);
    dropHeldDirect(entry.scope);
  };
  return (
    <div data-slot="held-direct" className="text-muted-foreground min-h-row flex items-center gap-2 px-3 text-sm">
      <WorkerGlyph taskId={entry.taskId} />
      <span className="min-w-0 flex-1 truncate">Not sent to {name ?? "the worker"}</span>
      <Button size="xs" variant="ghost" onClick={restore}>
        Restore
      </Button>
      <Button size="xs" variant="ghost" onClick={() => dropHeldDirect(entry.scope)}>
        Discard
      </Button>
    </div>
  );
}
