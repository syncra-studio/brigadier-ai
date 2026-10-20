import type { FC } from "react";

import { useAction } from "@/app/conversation/useAction";
import { Spinner } from "@/components/glyphs/spinner";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { BranchAlt } from "@/components/ui/icon";
import type { ConversationKind, ForkPlace } from "@/ipc/generated";
import { forkConversation } from "@/state/actions";

export const FORK_PLACES: readonly { place: ForkPlace; title: string; detail: string }[] = [
  {
    place: "workspace",
    title: "Fork in the same folder",
    detail: "Fork from this message in this session’s folder",
  },
  {
    place: "newWorktree",
    title: "Fork in a new worktree",
    detail: "Fork from this message in a new worktree",
  },
];

/**
 * "Fork chat from here" under an answer. A session asks where the fork works (its checkout
 * or a new worktree, from the commit current at this answer); a Chat, which works nowhere,
 * forks at once.
 */
export const ForkMenu: FC<{ conversationId: string; kind: ConversationKind; messageId: string }> = ({
  conversationId,
  kind,
  messageId,
}) => {
  const action = useAction();
  const fork = (place: ForkPlace) =>
    action.run(() => forkConversation(conversationId, messageId, place));
  const button = (
    <TooltipIconButton
      tooltip="Fork chat from here"
      disabled={action.busy}
      aria-busy={action.busy || undefined}
      onClick={kind === "chat" ? () => fork("workspace") : undefined}
    >
      {/* Spinning while the fork is made. */}
      {action.busy ? <Spinner className="animate-spin" /> : <BranchAlt />}
    </TooltipIconButton>
  );
  const error = action.error && (
    <span role="alert" className="text-destructive text-xs">
      {action.error}
    </span>
  );
  if (kind === "chat") {
    return (
      <>
        {button}
        {error}
      </>
    );
  }
  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>{button}</DropdownMenuTrigger>
        <ForkPlaces side="bottom" onPick={fork} />
      </DropdownMenu>
      {error}
    </>
  );
};

/** The fork menu's content: where a session's fork works. */
export const ForkPlaces: FC<{ side: "top" | "bottom"; onPick: (place: ForkPlace) => void }> = ({
  side,
  onPick,
}) => (
  <DropdownMenuContent side={side} align="start" className="w-sm p-2">
    <DropdownMenuLabel className="text-foreground px-2 pt-1 pb-2 font-sans text-base font-semibold tracking-normal normal-case">
      Fork chat from here
    </DropdownMenuLabel>
    {FORK_PLACES.map(({ place, title, detail }) => (
      <DropdownMenuItem key={place} className="items-start gap-3 py-2" onSelect={() => onPick(place)}>
        <BranchAlt className="mt-0.5" />
        <span className="flex flex-col gap-0.5">
          <span>{title}</span>
          <span className="text-muted-foreground text-xs">{detail}</span>
        </span>
      </DropdownMenuItem>
    ))}
  </DropdownMenuContent>
);
