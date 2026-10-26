import { useContext } from "react";

import { ComposerTargetContext } from "@/app/conversation/composerTarget";
import { composerPill } from "@/components/conversation";
import { PullRequestMerged as Merge } from "@/components/icons";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays";
import { cn } from "@/lib/utils";
import { NEW_CHAT_SCOPE } from "@/state/drafts";
import { armFusionSend, FUSION_HINT, isFusionChat, useFusionArmed, useFusionReady } from "@/state/fusionSend";

export function FusionChip() {
  const target = useContext(ComposerTargetContext);
  const scope = target?.conversation?.id ?? NEW_CHAT_SCOPE;
  const armed = useFusionArmed(scope);
  const ready = useFusionReady();
  const available = target?.conversation ? isFusionChat(target.conversation) : target?.resolved.kind === "chat";
  if (!available || !armed) return null;
  return (
    <>
      <span aria-hidden className="bg-foreground/15 mx-1 h-icon-md w-px" />
      <Tooltip>
        <TooltipTrigger asChild>
          <button type="button" aria-pressed data-slot="fusion-chip" className={cn(composerPill, "text-muted-foreground")} onClick={() => armFusionSend(scope, false)}>
            <Merge />Fusion
          </button>
        </TooltipTrigger>
        <TooltipContent side="bottom">{ready ? "Turn Fusion off" : `${FUSION_HINT}. Click to turn Fusion off.`}</TooltipContent>
      </Tooltip>
    </>
  );
}

export function FusionMessagePill() {
  return <span data-slot="fusion-message-pill" className={cn(composerPill, "text-muted-foreground mb-1 w-fit")}><Merge />Fusion</span>;
}
