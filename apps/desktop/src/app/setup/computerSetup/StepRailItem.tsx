import { Check } from "@/components/icons";
import { cn } from "@/lib/utils";

/** One step in the setup window's rail: its number, or a tick once done. */
export function StepRailItem({
  title,
  number,
  done,
  current,
}: {
  title: string;
  number: number;
  done: boolean;
  current: boolean;
}) {
  return (
    <div
      data-slot="setup-step"
      data-done={done || undefined}
      aria-current={current ? "step" : undefined}
      className={cn(
        "text-foreground/60 flex items-center gap-2 rounded-nav px-2 py-1.5 text-sm",
        current && "bg-foreground/5 text-foreground",
      )}
    >
      <span
        className={cn(
          "flex size-5 shrink-0 items-center justify-center rounded-full border text-xs",
          done ? "bg-success/20 text-success border-transparent" : "border-divider",
        )}
      >
        {done ? <Check aria-label="Done" className="size-icon-xs" /> : number}
      </span>
      {title}
    </div>
  );
}
