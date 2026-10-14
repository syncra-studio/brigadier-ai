import { ChevronDown } from "@openai/apps-sdk-ui/components/Icon";
import { useEffect, useMemo, useRef, useState } from "react";

import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { PreviewList } from "@/app/conversation/PreviewList";
import { previewChip } from "@/app/conversation/previewStatus";
import { Button } from "@/components/ui/button";
import { SummarySection } from "@/components/assistant-ui/elements/summary-section";
import { AgentStatus } from "@/components/assistant-ui/elements/agent-status";
import { formatDuration } from "@/lib/format";
import { useBoard } from "@/state/board";

/** The preview list opens from its chip, anchored inside the context card. */
export function PreviewChip({ conversationId }: { conversationId: string }) {
  const previews = useBoard((s) => (s.board?.conversationId === conversationId ? s.board.previews : null));
  const chip = useMemo(() => (previews ? previewChip(previews) : null), [previews]);
  const trigger = useRef<HTMLButtonElement | null>(null);
  const card = useRef<HTMLElement | null>(null);
  const [open, setOpen] = useState(false);
  const [now, setNow] = useState(Date.now);
  const active = chip?.active ?? false;
  useEffect(() => {
    // Synchronize the clock immediately when restarting its external timer.
    // eslint-disable-next-line react/set-state-in-effect
    setNow(Date.now());
    if (!active) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [active]);
  // Cleared while open: the next preview's chip comes back closed.
  if (!chip && open) setOpen(false);
  if (!chip) return null;
  return (
    <SummarySection data-activities foldKey="activities" title="Activities" count={Object.keys(previews ?? {}).length} unfoldOnIncrease={Object.keys(previews ?? {}).length}>
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <Button
            ref={trigger}
            variant="ghost"
            size="xs"
            data-slot="preview-chip"
            title={chip.title}
            aria-label={`Show previews: ${chip.label}, ${chip.status}`}
            aria-expanded={open}
            className="h-auto w-fit max-w-full rounded-capsule p-0 font-normal"
          >
            <AgentStatus state={chip.indicator} label={chip.label} trailing={<ChevronDown className="size-icon-xs" />}
              elapsed={chip.active ? formatDuration(Math.max(0, now - chip.startedAtMs)) : undefined} />
          </Button>
        </PopoverTrigger>
        <PopoverContent
          onOpenAutoFocus={() => {
            card.current = trigger.current?.closest<HTMLElement>('[data-slot="summary-card"]') ?? null;
          }}
          onCloseAutoFocus={(event) => {
            const board = useBoard.getState().board;
            if (board?.conversationId === conversationId && !Object.keys(board.previews).length && card.current?.isConnected) {
              event.preventDefault();
              card.current.focus({ preventScroll: true });
            }
          }}
          align="end" aria-label="Previews" className="flex max-h-(--radix-popover-content-available-height) w-96 max-w-(--radix-popover-content-available-width) flex-col overflow-y-auto rounded-2xl border-0 p-0">
          <PreviewList conversationId={conversationId} onOpenLink={() => setOpen(false)} />
        </PopoverContent>
      </Popover>
    </SummarySection>
  );
}
