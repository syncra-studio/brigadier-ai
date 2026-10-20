import { cn } from "@/lib/utils";

const TONES = { ready: "bg-success", needs: "bg-warning", off: "bg-muted-foreground" } as const;

/**
 * A small round dot before a line: a state's `tone` (ready, needing a step, or off), or any
 * `color` class (a storage part's own).
 */
export function StateDot({ tone = "off", color }: { tone?: keyof typeof TONES; color?: string }) {
  return <span aria-hidden className={cn("size-1.5 shrink-0 rounded-full", color ?? TONES[tone])} />;
}
