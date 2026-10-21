import { cn } from "@/lib/utils";
import type { RowTone } from "@/state/browserImport";

const DOT: Record<RowTone, string> = {
  ready: "bg-success",
  attention: "bg-warning",
  idle: "border-foreground/45 border",
};

/** Where a browser stands in Import Browser Data: a dot of its tone and the words. */
export function BrowserStatus({ tone, status }: { tone: RowTone; status: string }) {
  return (
    <span data-slot="import-status" className={cn("inline-flex shrink-0 items-center gap-1.5", tone === "attention" && "text-warning")}>
      <span aria-hidden className={cn("size-1.5 shrink-0 rounded-full", DOT[tone])} />
      {status}
    </span>
  );
}
