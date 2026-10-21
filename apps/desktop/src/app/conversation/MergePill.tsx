import type { FC } from "react";

import { useAction } from "@/app/conversation/useAction";
import { Button } from "@/components/controls";
import { Branch } from "@/components/icons";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays";
import { cancelMerge } from "@/state/actions";

/**
 * "Merging after 2 workers finish", with Cancel: the user's yes, held while workers write. While
 * a merge card waits for their answer, the card is where they say it: no Cancel.
 */
export const MergePill: FC<{ conversationId: string; words: string; branch: string; base: string; asking: boolean }> = ({ conversationId, words, branch, base, asking }) => {
  const cancel = useAction();
  return (
    <div data-slot="merge-pill" className="flex flex-col px-1 py-1">
      <div className="flex min-h-row items-center gap-1">
        <Tooltip>
          <TooltipTrigger asChild>
            <span className="text-foreground/60 flex min-w-0 flex-1 items-center gap-1.5 px-2 py-1 text-sm">
              <Branch aria-hidden className="size-4 shrink-0" />
              <span className="min-w-0 truncate">{words}</span>
            </span>
          </TooltipTrigger>
          <TooltipContent side="top">
            {asking ? `Merges ${branch} into ${base} once you say yes on its card` : `Merges ${branch} into ${base} once they’re done, as you asked`}
          </TooltipContent>
        </Tooltip>
        {!asking && (
          <Button
            size="xs"
            variant="ghost"
            data-slot="merge-cancel"
            disabled={cancel.busy}
            onClick={() => cancel.run(() => cancelMerge(conversationId))}
            className="text-foreground/60 hover:text-foreground"
          >
            Cancel
          </Button>
        )}
      </div>
      {cancel.error && <p role="alert" className="text-destructive px-2 text-xs">{cancel.error}</p>}
    </div>
  );
};
