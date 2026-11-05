import { useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { ComposerRailItem } from "@/components/conversation";
import { Button } from "@/components/controls";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/overlays";
import type { ModelGroup } from "@/components/shared";
import { moveNow } from "@/state/actions";
import { useCliThread } from "@/state/cliThread";

/**
 * A switch to another agent that waits for the session's background jobs says so above the
 * composer (#794), and Switch now goes without them, after a warning: no job ends unannounced.
 */
export function MoveWaitNotice({ conversationId, groups }: { conversationId: string; groups: readonly ModelGroup[] }) {
  const to = useCliThread((s) => s.views[conversationId]?.jobs?.moveTo ?? null);
  const [asking, setAsking] = useState(false);
  // Per switch: the row is gone when it comes or is called off.
  const [goingTo, setGoingTo] = useState<string | null>(null);
  const action = useAction();
  if (to === null) return null;
  const name = groups.find((group) => group.provider === to)?.label ?? to;
  const going = goingTo === to && action.error === null;
  const go = () => {
    setAsking(false);
    setGoingTo(to);
    action.run(() => moveNow(conversationId));
  };
  return (
    <ComposerRailItem label="Switch waiting">
      <div data-slot="move-wait" className="text-muted-foreground min-h-row flex items-center gap-2 px-3 text-sm">
        <span className="min-w-0 flex-1 truncate">
          {action.error ?? (going ? `Switching to ${name}…` : `Switching to ${name} once background jobs end`)}
        </span>
        {!going && (
          <Button size="xs" variant="ghost" onClick={() => setAsking(true)}>
            Switch now
          </Button>
        )}
      </div>
      <Dialog open={asking} onOpenChange={setAsking}>
        <DialogContent className="max-w-md">
          <DialogHeader>
            <DialogTitle>Switch to {name} now?</DialogTitle>
            <DialogDescription>
              Background jobs are still running. Switching now closes the current agent, which may end them.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={() => setAsking(false)}>
              Keep waiting
            </Button>
            <Button type="button" onClick={go}>
              Switch now
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </ComposerRailItem>
  );
}
