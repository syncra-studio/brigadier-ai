import { lazy, Suspense, useMemo, useState } from "react";

import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { previewChip } from "@/app/conversation/previewStatus";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";

const RunningTab = lazy(() => import("@/app/conversation/RunningTab").then((m) => ({ default: m.RunningTab })));

/** The preview list opens from its chip, anchored inside the context card. */
export function PreviewChip({ conversationId }: { conversationId: string }) {
  const previews = useBoard((s) => (s.board?.conversationId === conversationId ? s.board.previews : null));
  const chip = useMemo(() => (previews ? previewChip(previews) : null), [previews]);
  const [open, setOpen] = useState(false);
  // Cleared while open: the next preview's chip comes back closed.
  if (!chip && open) setOpen(false);
  if (!chip) return null;
  return (
    <div className="px-4 py-1.5">
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <Button
            variant="ghost"
            size="xs"
            data-slot="preview-chip"
            title={chip.title}
            aria-label={`Show previews: ${chip.label}, ${chip.status}`}
            aria-expanded={open}
            className={cn("h-pill rounded-capsule flex max-w-56 shrink-0 gap-1.5 px-2.5",
              chip.active ? "bg-success/15 text-success" : "bg-muted text-muted-foreground")}
          >
            <span className="min-w-0 truncate">{chip.label}</span>
            <span className="shrink-0">· {chip.status}</span>
          </Button>
        </PopoverTrigger>
        <PopoverContent align="end" aria-label="Previews" className="flex max-h-(--radix-popover-content-available-height) w-summary-stack max-w-(--radix-popover-content-available-width) flex-col overflow-y-auto p-0">
          <Suspense fallback={null}>
            <RunningTab conversationId={conversationId} onOpenLink={() => setOpen(false)} />
          </Suspense>
        </PopoverContent>
      </Popover>
    </div>
  );
}
