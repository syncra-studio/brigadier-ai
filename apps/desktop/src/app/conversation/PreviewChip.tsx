import { useEffect, useMemo, useState } from "react";

import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { PreviewList } from "@/app/conversation/PreviewList";
import { previewChip } from "@/app/conversation/previewStatus";
import { Button } from "@/components/ui/button";
import { AgentStatus } from "@/components/assistant-ui/elements/agent-status";
import { formatDuration } from "@/lib/format";
import { useBoard } from "@/state/board";


/** The preview list opens from its chip, anchored inside the context card. */
export function PreviewChip({ conversationId }: { conversationId: string }) {
  const previews = useBoard((s) => (s.board?.conversationId === conversationId ? s.board.previews : null));
  const chip = useMemo(() => (previews ? previewChip(previews) : null), [previews]);
  const [open, setOpen] = useState(false);
  const [now, setNow] = useState(Date.now);
  const active = chip?.active ?? false;
  useEffect(() => {
    if (!active) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [active]);
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
            className="h-pill max-w-full rounded-capsule p-0"
          >
            <AgentStatus state={chip.indicator} status={chip.status} label={chip.label}
              elapsed={chip.active ? formatDuration(Math.max(0, now - chip.startedAtMs)) : undefined} />
          </Button>
        </PopoverTrigger>
        <PopoverContent align="end" aria-label="Previews" className="flex max-h-(--radix-popover-content-available-height) w-summary-stack max-w-(--radix-popover-content-available-width) flex-col overflow-y-auto p-0">
          <PreviewList conversationId={conversationId} onOpenLink={() => setOpen(false)} />
        </PopoverContent>
      </Popover>
    </div>
  );
}
