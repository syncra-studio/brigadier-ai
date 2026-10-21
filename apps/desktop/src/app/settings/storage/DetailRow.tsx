import { size, type Part } from "@/app/settings/storage/model";
import { StateDot } from "@/components/shared";
import { cn } from "@/lib/utils";

/** One part of what an opened row holds: its dot, name, line and size. */
export function DetailRow({ part, line }: { part: Part; line: string }) {
  return (
    <div data-part-row={part.key} className="border-divider flex items-center gap-3 border-t py-2.5">
      <StateDot color={part.color} />
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="text-foreground text-xs font-medium">{part.label}</span>
        <span className="text-foreground/65 text-xs">{line}</span>
      </div>
      <span
        className={cn(
          "w-18 shrink-0 text-right text-xs",
          part.bytes > 0 ? "text-foreground" : "text-muted-foreground",
        )}
      >
        {size(part.bytes)}
      </span>
    </div>
  );
}
