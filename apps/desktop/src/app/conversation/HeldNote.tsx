import type { FC } from "react";

import { elapsedWords } from "@/app/conversation/liveStatus";
import { useNow } from "@/hooks/use-now";
import type { HeldBy } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/** When a message sent into the turn is read: once the thread's running command ends. */
export function heldWords(held: HeldBy, now: number): string {
  return `Read when the current command ends · running ${elapsedWords(now - held.sinceMs)}`;
}

/**
 * Under a message the thread can't read yet (it is inside a command): when it will be, and how
 * long that command has run, so the user knows the message isn't ignored.
 */
export const HeldNote: FC<{ held: HeldBy; className?: string }> = ({ held, className }) => {
  const now = useNow(5_000);
  return (
    <span
      data-slot="held-note"
      title={`The thread is running ${held.command} and reads this once it ends`}
      className={cn("text-muted-foreground text-xs", className)}
    >
      {heldWords(held, now)}
    </span>
  );
};
