import { Globe } from "@/components/icons";
import type { ImportSource } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/** A browser's app icon, or a globe in a tile when it has none. */
export function BrowserIcon({ source, className }: { source: ImportSource; className?: string }) {
  return source.icon ? (
    <img src={source.icon} alt="" aria-hidden className={cn("size-8 shrink-0", className)} />
  ) : (
    <span aria-hidden className={cn("bg-foreground/5 text-foreground/65 flex size-8 shrink-0 items-center justify-center rounded-lg", className)}>
      <Globe className="size-icon-md" />
    </span>
  );
}
