import type { ComponentProps } from "react";

import { field, ghostButton, mono } from "@/components/conversation/elements/surfaces";
import { Brain, X } from "@/components/icons";
import { cn } from "@/lib/utils";

export type MemoryChipChange = "added" | "updated" | "existing";

export interface MemoryChip {
  id: string;
  text: string;
  change: MemoryChipChange;
}

/**
 * The Memory chips element (assistant-ui), on Brigadier's tokens: what the model now remembers
 * about the user, written during the turn and removable.
 */
export function MemoryChips({
  chips,
  onForget,
  className,
  ...props
}: Omit<ComponentProps<"div">, "children" | "chips" | "onForget"> & {
  chips: readonly MemoryChip[];
  onForget?: (id: string) => void;
}) {
  const fresh = chips.filter((chip) => chip.change !== "existing").length;

  return (
    <div
      data-slot="memory-chips"
      className={cn("flex w-full max-w-sm flex-col gap-2", className)}
      {...props}
    >
      <div className="flex items-center gap-1.5">
        <Brain className="text-foreground/30 size-icon-sm" />
        <span className={cn(mono, "text-foreground/35")}>
          {fresh > 0 ? `remembered ${fresh}` : "memory"}
        </span>
      </div>

      <div className="flex flex-wrap gap-1.5">
        {chips.map((chip) => (
          <span
            key={chip.id}
            className={cn(
              "fade-in zoom-in-95 animate-in fill-mode-both group rounded-capsule flex items-center gap-1 py-1 ps-2.5 pe-1 text-xs duration-300",
              chip.change === "existing" ? cn(field, "text-foreground/55") : "bg-link/15 text-link",
              !onForget && "pe-2.5",
            )}
          >
            {chip.text}
            {onForget && (
              <button
                type="button"
                aria-label={`Forget "${chip.text}"`}
                onClick={() => onForget(chip.id)}
                className={cn(ghostButton, "size-icon-md")}
              >
                <X className="size-icon-xs" />
              </button>
            )}
          </span>
        ))}
      </div>
    </div>
  );
}
